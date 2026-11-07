// SPDX-License-Identifier: CC0-1.0

//! This front door's AUDIT leg: every call it serves is a management event of
//! `elasticloadbalancing.amazonaws.com`, recorded in the account's event history where
//! CloudTrail's `LookupEvents` reads it.
//!
//! Every control-plane call of an account is a management event, which makes the recording
//! a property of SERVING a call and not of any one operation — so it is not written per
//! handler here. The shared wrapper ([`service_common::audited_operation`]) is handed the
//! deserialized input before the handler runs, the handler's own `Result` after, and the
//! rendered response last, and this module is only what THIS service contributes to each
//! record:
//!
//! * its `eventSource` ([`EVENT_SOURCE`]);
//! * the Region and the account the call was served for;
//! * the request members whose values its records WITHHOLD ([`SENSITIVE_MEMBERS`]); and
//! * which operations REPORT state and which CHANGE it — the one fact a record cannot read
//!   off a request, so it is stated per operation in [`audited_table`].
//!
//! Every request this service serves reaches that table, because it is the routing table
//! this service's definition answers with
//! ([`service_common::audited_operation::AuditedDefinition`]) — so neither the request that
//! arrives under this model's own routing key nor the one the shared Elastic Load Balancing
//! front door hands over can be served by a path that records nothing.
//!
//! A call this emulator has not implemented answers 501 and is NOT recorded: the wrapper
//! drops it, because event history is the record of the calls an account's APIs actually
//! served.

use std::sync::Arc;

use service_common::audited_operation::{AuditedService, AuditedTable, CallKind};
use service_common::management_events::{ManagementEventLog, RecordedSession, ServedApiCall};
use smithy_server_runtime::{OperationContext, OperationRoutingTable, PreDispatchProtocol};

/// The `eventSource` every record of this service carries.
///
/// The model's own answer and not a name assembled here:
/// `smithy_query(query="service_info")` reports
/// `awsService.cloudTrailEventSource: elasticloadbalancing.amazonaws.com` (the same
/// `<endpointPrefix>.amazonaws.com` form CloudTrail stamps on every event). It is the
/// source of BOTH Elastic Load Balancing APIs, which share that endpoint prefix.
pub const EVENT_SOURCE: &str = "elasticloadbalancing.amazonaws.com";

/// The request members this service's records carry CloudTrail's redaction literal for
/// instead of the value the caller sent.
///
/// The OAuth 2.0 client secret of an `authenticate-oidc` action, which `CreateRule`,
/// `ModifyRule`, `CreateListener` and `ModifyListener` each carry inside
/// `Actions.AuthenticateOidcConfig` (the member's own documentation: "The OAuth 2.0 client
/// secret"). It authenticates this account to the identity provider, so the value is
/// replaced before anything is stored and the plaintext is retrievable from event history
/// nowhere. The set is by member NAME, so it holds for every operation that carries a
/// member of that name — the secret on a create and the same secret on a modify are both
/// withheld.
pub const SENSITIVE_MEMBERS: &[&str] = &["ClientSecret"];

/// What this service contributes to the record of each call it serves.
///
/// Holds the shared engine — the record goes into the log that engine carries, the one
/// CloudTrail reads — so it is generic over the engine type exactly as the service is.
pub struct LoadBalancingAudit<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> LoadBalancingAudit<Eng> {
    /// Build the recorder from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// The account's log in that engine, or `None` for an engine that carries none.
    fn log(&self) -> Option<::engine::ServiceState<ManagementEventLog>> {
        crate::state::management_event_log(&*self.engine)
    }
}

impl<Eng: Send + Sync + 'static> AuditedService for LoadBalancingAudit<Eng> {
    fn record(
        &self,
        call: ServedApiCall,
        answered_as: &str,
    ) -> Option<::service_common::api_call_events::DeliveredApiCall> {
        let Some(log) = self.log() else {
            // Not silent: an unrecorded call leaves no trace in the log by definition, so
            // it leaves one here instead.
            ::tracing::warn!(
                operation = call.event_name,
                "the engine carries no management-event log: the call was served unrecorded"
            );
            return None;
        };
        // The record, and the event the account's CloudTrail fans out for it while a trail
        // of the Region is logging calls of its kind; publishing it is the wrapper's.
        // The guard is BOUND rather than used as a temporary: a local is released before
        // the handle it borrows, which a temporary at the end of this block would not be.
        let mut recording = log.lock();
        recording.record_delivered(
            call,
            ::std::time::SystemTime::now(),
            answered_as.to_string(),
        )
    }

    fn recorded_session(&self, access_key_id: &str) -> Option<RecordedSession> {
        // The account's own session directory, so a call made with a role session is
        // recorded as that session: the shared store the minting service wrote it to.
        crate::state::session_audit_directory(&*self.engine)
            .and_then(|directory| directory.lock().session(access_key_id).cloned())
    }

    fn event_source(&self) -> &'static str {
        EVENT_SOURCE
    }

    fn region(&self, ctx: &OperationContext<'_>) -> String {
        // The Region the call was served in, read the way every handler of this service
        // reads it — which is also the Region of every ARN the call mints.
        crate::state::request_region(ctx)
    }

    fn account_id(&self) -> String {
        ::emulator_server::emulator_account_id().to_string()
    }

    // `resources` is the shared recorder's default: none. The pairs CloudTrail reports in
    // an event's `Resources` are not this fix's to invent — a record naming a resource
    // nothing measured would be reported to a lookup by `ResourceName` as fact — so a
    // record of a call to this service carries the call, its parameters and its outcome,
    // and no resource list.

    fn sensitive_members(&self) -> &'static [&'static str] {
        SENSITIVE_MEMBERS
    }
}

/// The envelope a pre-dispatch failure of one of this service's operations renders in:
/// this service speaks `awsQuery` (not the EC2 dialect), which is the same value the
/// generated per-operation shim passes.
const QUERY_ENVELOPE: PreDispatchProtocol<'static> = PreDispatchProtocol::Query { ec2: false };

/// A call that only REPORTS state, as the table's rows name that kind.
const fn reports() -> CallKind {
    CallKind::Reporting
}

/// A call that CHANGES the account's resources.
const fn mutates() -> CallKind {
    CallKind::Mutating
}

/// Serve one operation of this service audited, as a call of the stated kind: `reports`
/// for an operation that only REPORTS state, `mutates` for one that CHANGES the account's
/// resources — which is the event's `readOnly`, and what a lookup can filter on.
macro_rules! audited {
    (
        $table:expr, $engine:expr,
        $( $module:ident :: $operation:ident => $kind:ident ),* $(,)?
    ) => {{
        $(
            $table.serve(
                crate::ops::$module::$operation::new(::std::sync::Arc::clone($engine)),
                $kind(),
            );
        )*
    }};
}

/// Every modelled operation of this service, each constructed from the shared engine and
/// served behind the audit recorder.
///
/// EVERY one is here, the ones this emulator has not implemented included: a stub answers
/// 501 and the wrapper records nothing for it, so an operation implemented later is
/// auditable the moment it serves a call rather than when someone remembers to add it.
///
/// The kind per row is the operation's own semantics: `reports` for the `Describe*` and
/// `Get*` readers, which report state and change none of it, and `mutates` for every
/// operation that creates, changes, attaches or removes a resource.
pub fn audited_table<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) -> OperationRoutingTable {
    let recorder = Arc::new(LoadBalancingAudit::new(Arc::clone(engine)));
    let mut table = AuditedTable::new(recorder, QUERY_ENVELOPE);
    audited!(
        table, engine,
        add_listener_certificates::AddListenerCertificates => mutates,
        add_tags::AddTags => mutates,
        add_trust_store_revocations::AddTrustStoreRevocations => mutates,
        create_listener::CreateListener => mutates,
        create_load_balancer::CreateLoadBalancer => mutates,
        create_rule::CreateRule => mutates,
        create_target_group::CreateTargetGroup => mutates,
        create_trust_store::CreateTrustStore => mutates,
        delete_listener::DeleteListener => mutates,
        delete_load_balancer::DeleteLoadBalancer => mutates,
        delete_rule::DeleteRule => mutates,
        delete_shared_trust_store_association::DeleteSharedTrustStoreAssociation => mutates,
        delete_target_group::DeleteTargetGroup => mutates,
        delete_trust_store::DeleteTrustStore => mutates,
        deregister_targets::DeregisterTargets => mutates,
        describe_account_limits::DescribeAccountLimits => reports,
        describe_capacity_reservation::DescribeCapacityReservation => reports,
        describe_listener_attributes::DescribeListenerAttributes => reports,
        describe_listener_certificates::DescribeListenerCertificates => reports,
        describe_listeners::DescribeListeners => reports,
        describe_load_balancer_attributes::DescribeLoadBalancerAttributes => reports,
        describe_load_balancers::DescribeLoadBalancers => reports,
        describe_rules::DescribeRules => reports,
        describe_ssl_policies::DescribeSSLPolicies => reports,
        describe_tags::DescribeTags => reports,
        describe_target_group_attributes::DescribeTargetGroupAttributes => reports,
        describe_target_groups::DescribeTargetGroups => reports,
        describe_target_health::DescribeTargetHealth => reports,
        describe_trust_store_associations::DescribeTrustStoreAssociations => reports,
        describe_trust_store_revocations::DescribeTrustStoreRevocations => reports,
        describe_trust_stores::DescribeTrustStores => reports,
        get_resource_policy::GetResourcePolicy => reports,
        get_trust_store_ca_certificates_bundle::GetTrustStoreCaCertificatesBundle => reports,
        get_trust_store_revocation_content::GetTrustStoreRevocationContent => reports,
        modify_capacity_reservation::ModifyCapacityReservation => mutates,
        modify_ip_pools::ModifyIpPools => mutates,
        modify_listener::ModifyListener => mutates,
        modify_listener_attributes::ModifyListenerAttributes => mutates,
        modify_load_balancer_attributes::ModifyLoadBalancerAttributes => mutates,
        modify_rule::ModifyRule => mutates,
        modify_target_group::ModifyTargetGroup => mutates,
        modify_target_group_attributes::ModifyTargetGroupAttributes => mutates,
        modify_trust_store::ModifyTrustStore => mutates,
        register_targets::RegisterTargets => mutates,
        remove_listener_certificates::RemoveListenerCertificates => mutates,
        remove_tags::RemoveTags => mutates,
        remove_trust_store_revocations::RemoveTrustStoreRevocations => mutates,
        set_ip_address_type::SetIpAddressType => mutates,
        set_rule_priorities::SetRulePriorities => mutates,
        set_security_groups::SetSecurityGroups => mutates,
        set_subnets::SetSubnets => mutates,
    );
    table.finish()
}
