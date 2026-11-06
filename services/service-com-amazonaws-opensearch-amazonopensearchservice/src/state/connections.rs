// SPDX-License-Identifier: CC0-1.0

//! Cross-cluster connections.
//!
//! One [`Connection`] record carries both views of a connection — the source domain
//! owner's outbound one and the destination owner's inbound one — because they are one
//! resource sharing a `ConnectionId`: `CreateOutboundConnection` mints it,
//! `DescribeOutboundConnections` and `DescribeInboundConnections` report it, and the
//! accept, reject and delete operations move its status.
//!
//! The status values and the quotas the record is judged by live in [`crate::state`]
//! itself, beside the other catalogs and bounds every part of the store reads.

use super::*;

/// One cross-cluster connection between a source (local) domain and a destination
/// (remote) domain, as this emulator persists it.
///
/// A cross-cluster connection is ONE resource with two views: the source domain
/// owner's OUTBOUND connection and the destination owner's INBOUND connection, which
/// share a `ConnectionId` and move through the lifecycle together. This is the single
/// record behind both views — `CreateOutboundConnection` mints it,
/// `DescribeOutboundConnections` reports it as [`Connection::to_outbound`] and
/// `DescribeInboundConnections` reports it as [`Connection::to_inbound`] — so no two
/// operations can report the same connection differently.
#[derive(Debug, Clone)]
pub struct Connection {
    /// `OutboundConnection$ConnectionId`, which is also the inbound view's. Unique
    /// per connection, so it IS the key the store holds the record under.
    pub connection_id: String,
    /// The order the store minted this record in.
    ///
    /// The id is an opaque token, so it does not say which of two records is the
    /// older one; this does, which is what lets the store release the OLDEST deleted
    /// record when it keeps more history than [`CONNECTION_HISTORY`].
    pub sequence: u64,
    /// `OutboundConnection$ConnectionAlias`: the name the create gave the connection.
    pub connection_alias: String,
    /// The source domain this connection is from, as the create's `LocalDomainInfo`
    /// resolved to.
    pub local_domain: AWSDomainInformation,
    /// The destination domain this connection is to, as the create's
    /// `RemoteDomainInfo` resolved to.
    pub remote_domain: AWSDomainInformation,
    /// `OutboundConnection$ConnectionMode`: [`CONNECTION_MODE_DIRECT`] or
    /// [`CONNECTION_MODE_VPC_ENDPOINT`].
    pub connection_mode: String,
    /// `OutboundConnection$ConnectionProperties`, absent when the create carried
    /// none. Stored as the modelled shape rather than a second hand-written mirror
    /// of its members.
    pub connection_properties: Option<ConnectionProperties>,
    /// Where the connection is in its lifecycle: one of
    /// `OutboundConnectionStatusCode`'s values.
    pub status: String,
    /// `OutboundConnectionStatus$Message`: what there is to say about that status
    /// beyond the code — why a validation failed, say. `None` where the code says it
    /// all.
    pub status_message: Option<String>,
    /// The status move the clock owes this connection, if any is outstanding.
    pub owed: Option<OwedStatus>,
    /// The status the DESTINATION owner's view of this connection has come to rest
    /// in for good, where that is no longer the status the shared lifecycle reports;
    /// `None` while both views report the one lifecycle.
    ///
    /// The two views of a connection share a lifecycle until a decision the
    /// destination owner took is a record only THAT owner keeps: refusing a request
    /// is the destination owner's permanent record of the decision, so the source
    /// owner deleting its own view of the connection leaves that record exactly where
    /// it was rather than carrying it along to the deleted status
    /// ([`Connection::begin_delete`]).
    pub inbound_rest: Option<String>,
}

impl Connection {
    /// Materialise the status move the clock owes this connection, if `now` is past
    /// its due instant: a request that was being validated becomes one waiting for
    /// the remote domain owner's decision, or one whose validation failed.
    ///
    /// This is the only write a reporting operation makes, and it writes only what
    /// was already owed at the same instant had nobody called.
    ///
    /// A status the clock owes may itself owe the next one ([`connection_successor`]),
    /// so the moves are applied in a loop from the instant each FELL DUE rather than
    /// from `now`: a caller that reads a connection long after it was accepted sees
    /// the status it would have reached without them, not one step of its lifecycle
    /// per call.
    pub fn settle(&mut self, now: SystemTime) {
        while let Some(due) = self.owed.as_ref().map(|owed| owed.due) {
            if now < due {
                return;
            }
            let Some(owed) = self.owed.take() else {
                return;
            };
            self.status = owed.status;
            self.status_message = owed.message;
            self.owed = connection_successor(&self.status).map(|status| OwedStatus {
                status: status.to_string(),
                message: None,
                due: due + PROVISIONING,
            });
        }
    }

    /// Whether this connection has been deleted: the delete a caller asked for has
    /// been carried out and the record is the account's record OF the deletion rather
    /// than a connection it holds.
    ///
    /// What a deleted connection no longer holds is everything a live one does — the
    /// alias its source domain named it by and the slot it counted against
    /// [`CONNECTION_QUOTA`] — so a source domain can connect to the same destination
    /// under the same alias again.
    pub fn deleted(&self) -> bool {
        self.status == CONNECTION_STATUS_DELETED
    }

    /// Put this connection on its way out of the store, unless a delete already did.
    ///
    /// Taking the connection down is work the service does, so the record is in its
    /// deleting status when the call that asked for this returns and the clock owes it
    /// the deleted status it comes to rest in ([`connection_successor`]). A delete of
    /// a connection a delete already took does not restart that: the record stands as
    /// it is, which is what makes a repeat delete of an id this caller deleted the
    /// no-op it reports rather than a second deletion of one connection.
    ///
    /// The two views follow the deletion together only where both are views of a LIVE
    /// connection. Where the destination owner's side has already come to rest on its
    /// own account — it refused the request, so its record is the record OF that
    /// refusal — or where it never had a view of the connection at all, the inbound
    /// view stays exactly where it was ([`Connection::inbound_rest`]): deleting the
    /// source owner's view of a connection is not the destination owner taking a
    /// decision back.
    pub fn begin_delete(&mut self, now: SystemTime) {
        if self.status == CONNECTION_STATUS_DELETING || self.deleted() {
            return;
        }
        if !self.has_inbound_view() || self.status == CONNECTION_STATUS_REJECTED {
            self.inbound_rest = Some(self.status.clone());
        }
        self.status = CONNECTION_STATUS_DELETING.to_string();
        self.status_message = None;
        self.owed = Some(OwedStatus {
            status: CONNECTION_STATUS_DELETED.to_string(),
            message: None,
            due: now + PROVISIONING,
        });
    }

    /// This record as `OutboundConnection` — the element shape
    /// `DescribeOutboundConnections` reports, and member-for-member the projection
    /// `CreateOutboundConnection` answers with.
    pub fn to_outbound(&self) -> OutboundConnection {
        OutboundConnection {
            local_domain_info: Some(domain_information(&self.local_domain)),
            remote_domain_info: Some(domain_information(&self.remote_domain)),
            connection_id: Some(self.connection_id.clone()),
            connection_alias: Some(self.connection_alias.clone()),
            connection_status: Some(OutboundConnectionStatus {
                status_code: Some(self.status.clone()),
                // "Verbose information for the outbound connection": carried only
                // where the status has something to say the code does not.
                message: self.status_message.clone(),
            }),
            connection_mode: Some(self.connection_mode.clone()),
            connection_properties: self.connection_properties.clone(),
        }
    }

    /// Where the DESTINATION owner's view of this connection is: the status its own
    /// side came to rest in for good, where that is no longer the shared lifecycle's
    /// ([`Connection::inbound_rest`]), and the shared lifecycle's status otherwise.
    ///
    /// Every operation on the destination owner's view of a connection decides from
    /// this rather than from [`Connection::status`], so what that owner is told about
    /// the connection and what it may do with it come from the same status.
    pub fn inbound_status(&self) -> &str {
        self.inbound_rest.as_deref().unwrap_or(&self.status)
    }

    /// Whether this connection has an INBOUND view at all — whether the destination
    /// domain's owner has anything to see or decide about it yet.
    ///
    /// The two views of one connection share a lifecycle, but not every status the
    /// source owner's outbound view reports is one the destination owner's inbound
    /// view can be in: `InboundConnectionStatusCode` carries neither
    /// [`CONNECTION_STATUS_VALIDATING`] nor [`CONNECTION_STATUS_VALIDATION_FAILED`],
    /// while `OutboundConnectionStatusCode` carries both. A request that is still
    /// being validated, or whose validation failed, therefore has no inbound view —
    /// it is reported to the source owner only, and the status set the inbound shape
    /// models is what says so.
    pub fn has_inbound_view(&self) -> bool {
        INBOUND_CONNECTION_STATUS_CODE_VALUES.contains(&self.inbound_status())
    }

    /// This record as `InboundConnection` — the element shape
    /// `DescribeInboundConnections` reports, and the projection every operation on
    /// the destination owner's view of a connection answers with.
    ///
    /// The same stored record [`Connection::to_outbound`] projects, so the two views
    /// of one connection cannot disagree about its id, its endpoints or its mode, and
    /// they report one lifecycle for as long as the connection has one
    /// ([`Connection::inbound_status`]). The inbound shape models no `ConnectionAlias`
    /// and no `ConnectionProperties`: the alias is how the SOURCE domain names the
    /// remote cluster, so it is the source owner's view that carries it.
    pub fn to_inbound(&self) -> InboundConnection {
        InboundConnection {
            local_domain_info: Some(domain_information(&self.local_domain)),
            remote_domain_info: Some(domain_information(&self.remote_domain)),
            connection_id: Some(self.connection_id.clone()),
            connection_status: Some(InboundConnectionStatus {
                status_code: Some(self.inbound_status().to_string()),
                // "Information about the connection": carried only where the status
                // has something to say the code itself does not.
                message: self.status_message.clone(),
            }),
            connection_mode: Some(self.connection_mode.clone()),
        }
    }
}

/// The status a connection moves to ON ITS OWN once it has reached `status`, and
/// `None` where it is somewhere only another call takes it further from.
///
/// A connection's lifecycle has steps the service drives and steps a caller drives:
/// once a domain owner has decided about a request, carrying that decision out is
/// work the service does, so each of those steps falls due [`PROVISIONING`] after the
/// previous one rather than waiting for another call. Where a connection waits for a
/// decision — a validated request waiting to be accepted or rejected — or where it
/// has come to rest, nothing is owed.
pub fn connection_successor(status: &str) -> Option<&'static str> {
    match status {
        CONNECTION_STATUS_PROVISIONING => Some(CONNECTION_STATUS_ACTIVE),
        CONNECTION_STATUS_REJECTING => Some(CONNECTION_STATUS_REJECTED),
        CONNECTION_STATUS_DELETING => Some(CONNECTION_STATUS_DELETED),
        _ => None,
    }
}

/// A stored domain reference in the container the wire reports it in.
pub fn domain_information(info: &AWSDomainInformation) -> DomainInformationContainer {
    DomainInformationContainer {
        aws_domain_information: Some(info.clone()),
    }
}

/// The values of a domain's `DomainConfig` options, as ONE snapshot: the
/// configuration a change started FROM, which is what cancelling that change puts
/// back.
///
/// Every member is the [`Domain`] field holding one option an `UpdateDomainConfig`
/// request can set, so a snapshot carries the whole configuration a change could
/// have moved and nothing else: the domain's identity, its history and the
/// resources it holds are not options and are never restored.
#[derive(Debug, Clone)]
pub struct DomainOptions {
    pub(super) cluster_config: ClusterConfig,
    pub(super) ebs_options: Option<EBSOptions>,
    pub(super) access_policies: Option<String>,
    pub(super) ip_address_type: Option<String>,
    pub(super) snapshot_options: Option<SnapshotOptions>,
    pub(super) vpc_options: Option<VPCDerivedInfo>,
    pub(super) cognito_options: Option<CognitoOptions>,
    pub(super) encryption_at_rest_options: Option<EncryptionAtRestOptions>,
    pub(super) node_to_node_encryption_options: Option<NodeToNodeEncryptionOptions>,
    pub(super) advanced_options: BTreeMap<String, String>,
    pub(super) log_publishing_options: BTreeMap<String, LogPublishingOption>,
    pub(super) domain_endpoint_options: Option<DomainEndpointOptions>,
    pub(super) advanced_security_options: Option<AdvancedSecurityOptions>,
    pub(super) identity_center_options: Option<IdentityCenterOptions>,
    pub(super) auto_tune_options: Option<AutoTuneOptions>,
    pub(super) off_peak_window_options: Option<OffPeakWindowOptions>,
    pub(super) software_update_options: Option<SoftwareUpdateOptions>,
    pub(super) aiml_options: Option<AIMLOptionsOutput>,
    pub(super) deployment_strategy_options: Option<DeploymentStrategyOptions>,
    pub(super) automated_snapshot_pause_options: Option<AutomatedSnapshotPauseOptions>,
    pub(super) use_case: String,
    pub(super) engine_mode: String,
}

impl DomainOptions {
    /// The option values `domain` holds right now.
    pub fn capture(domain: &Domain) -> Self {
        Self {
            cluster_config: domain.cluster_config.clone(),
            ebs_options: domain.ebs_options.clone(),
            access_policies: domain.access_policies.clone(),
            ip_address_type: domain.ip_address_type.clone(),
            snapshot_options: domain.snapshot_options.clone(),
            vpc_options: domain.vpc_options.clone(),
            cognito_options: domain.cognito_options.clone(),
            encryption_at_rest_options: domain.encryption_at_rest_options.clone(),
            node_to_node_encryption_options: domain.node_to_node_encryption_options.clone(),
            advanced_options: domain.advanced_options.clone(),
            log_publishing_options: domain.log_publishing_options.clone(),
            domain_endpoint_options: domain.domain_endpoint_options.clone(),
            advanced_security_options: domain.advanced_security_options.clone(),
            identity_center_options: domain.identity_center_options.clone(),
            auto_tune_options: domain.auto_tune_options.clone(),
            off_peak_window_options: domain.off_peak_window_options.clone(),
            software_update_options: domain.software_update_options.clone(),
            aiml_options: domain.aiml_options.clone(),
            deployment_strategy_options: domain.deployment_strategy_options.clone(),
            automated_snapshot_pause_options: domain.automated_snapshot_pause_options.clone(),
            use_case: domain.use_case.clone(),
            engine_mode: domain.engine_mode.clone(),
        }
    }

    /// Put these values back on `domain`, leaving everything that is not one of its
    /// options alone.
    pub fn restore(self, domain: &mut Domain) {
        domain.cluster_config = self.cluster_config;
        domain.ebs_options = self.ebs_options;
        domain.access_policies = self.access_policies;
        domain.ip_address_type = self.ip_address_type;
        domain.snapshot_options = self.snapshot_options;
        domain.vpc_options = self.vpc_options;
        domain.cognito_options = self.cognito_options;
        domain.encryption_at_rest_options = self.encryption_at_rest_options;
        domain.node_to_node_encryption_options = self.node_to_node_encryption_options;
        domain.advanced_options = self.advanced_options;
        domain.log_publishing_options = self.log_publishing_options;
        domain.domain_endpoint_options = self.domain_endpoint_options;
        domain.advanced_security_options = self.advanced_security_options;
        domain.identity_center_options = self.identity_center_options;
        domain.auto_tune_options = self.auto_tune_options;
        domain.off_peak_window_options = self.off_peak_window_options;
        domain.software_update_options = self.software_update_options;
        domain.aiml_options = self.aiml_options;
        domain.deployment_strategy_options = self.deployment_strategy_options;
        domain.automated_snapshot_pause_options = self.automated_snapshot_pause_options;
        domain.use_case = self.use_case;
        domain.engine_mode = self.engine_mode;
    }

    /// `option`'s value as the string `CancelledChangeProperty$CancelledValue` and
    /// `$ActiveValue` are, for an option whose value IS one.
    ///
    /// `None` for the structured containers and the maps: how this service renders
    /// one of them as a single string is not published, and an unpublished rendering
    /// is not invented — the property is then reported by name alone.
    pub fn option_value(&self, option: &str) -> Option<String> {
        match option {
            OPT_ACCESS_POLICIES => self.access_policies.clone(),
            OPT_IP_ADDRESS_TYPE => self.ip_address_type.clone(),
            OPT_USE_CASE => Some(self.use_case.clone()),
            OPT_ENGINE_MODE => Some(self.engine_mode.clone()),
            _ => None,
        }
    }
}
