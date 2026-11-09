// SPDX-License-Identifier: CC0-1.0

//! Whether a load balancer of this service is a MANAGED RESOURCE of Amazon Application
//! Recovery Controller, as the service that REPORTS managed resources has to see it.
//!
//! Turning zonal-shift integration on for a load balancer is a call to THIS service — the
//! `zonal_shift.config.enabled` attribute of `ModifyLoadBalancerAttributes` — and what it
//! makes true is a fact about ARC: the load balancer becomes one of the resources ARC
//! manages, so ARC reports it, reports the Availability Zones it is deployed in as the key
//! set of its applied weights, and lets a caller start a zonal shift for it. ARC models no
//! operation that registers a resource and cannot see this store, so the fact has to be
//! written where it can read it — the ONE shared registry
//! ([`::service_common::arc_managed_resources::ManagedResourceRegistry`]), which the engine
//! keys by TYPE, so a copy of its own in this service's state would be invisible to the
//! reader.
//!
//! The registration is DERIVED from the store rather than accumulated
//! ([`ElbState::zonal_shift_registration`]), exactly as the certificate consumers a load
//! balancer's listeners make are ([`super::record_certificate_consumers`]): the integration
//! turned off, the zone set changed by `SetSubnets` and the load balancer deleted each
//! record what the store holds NOW, so no stale registration can survive a call.
//!
//! A ZONE HAS TWO IDENTIFIERS. ARC names a zone the way its own API does — "The
//! Availability Zone (for example, `use1-az1`)" (`StartZonalShiftRequest$awayFrom`) — which
//! is the account-independent zone ID, while a load balancer's record holds the zone NAME
//! its subnet put it in. Only EC2 holds the pairing, so the IDs are read from EC2 for the
//! load balancer's own subnets ([`::crate::state::network::zone_ids_of_subnets`]) and the
//! zone name is what is recorded for a subnet EC2 reports no ID for.
//!
//! ONLY THIS SERVICE'S OWN ARNS. The registry is shared with every other service in the
//! process, so what is published is only ever a load balancer THIS store holds: a
//! registration is read off the record ([`ElbState::zonal_shift_registration`]), and an
//! identifier the store holds no load balancer for is [`ZonalShiftRegistration::Untracked`]
//! — recorded as nothing at all — so no identifier a request carried can reach the registry
//! and name an entry that is not this service's.
//!
//! LOCK ORDER: the registry is taken on its own here, after this service's store has been
//! read and released — the EC2 read is awaited in between — so nothing holds the registry
//! while taking another store.

use super::{
    ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State as ElbState, LoadBalancerRecord,
};

/// The attribute whose documentation turns zonal-shift integration on for a load balancer:
/// "zonal_shift.config.enabled - Indicates whether zonal shift is enabled. The possible
/// values are true and false. The default is false." The catalog
/// (`super::LOAD_BALANCER_ATTRIBUTES`) is the one source of that default, here as in the
/// attribute collection a read reports.
pub const ZONAL_SHIFT_ATTRIBUTE: &str = "zonal_shift.config.enabled";

/// The SHARED managed-resource registry in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// Answer `None` by leaving the registry unwritten: the record is an aid to the service
/// that reports the managed resource, never part of this operation's own answer, so an
/// engine carrying no registry still serves the call.
pub fn managed_resource_registry<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::arc_managed_resources::ManagedResourceRegistry>>
{
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::arc_managed_resources::ManagedResourceRegistry,
            >,
        )
}

/// One Availability Zone a load balancer is deployed in, as this store holds it: the
/// subnet that put it there and the zone's NAME.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZonePlacement {
    pub subnet_id: String,
    pub zone_name: String,
}

/// What a load balancer is to ARC right now, read off this service's store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZonalShiftRegistration {
    /// A load balancer with zonal-shift integration enabled: a managed resource of ARC,
    /// deployed in these zones.
    Managed {
        arn: String,
        name: String,
        zones: Vec<ZonePlacement>,
    },
    /// A load balancer THIS STORE HOLDS that is no managed resource: its integration is
    /// off, or its family carries no such attribute at all.
    Withdrawn { arn: String },
    /// This store holds no load balancer for the identifier at all, so the identifier is a
    /// value out of the request and nothing about it is this service's to publish.
    Untracked,
}

impl ZonalShiftRegistration {
    /// What this registration becomes once the load balancer it describes is REMOVED: a
    /// resource that is gone is a managed resource of nothing.
    ///
    /// An identifier this store never held stays [`Untracked`], which is what keeps the
    /// idempotent delete of an absent load balancer from naming an entry of the shared
    /// registry it has nothing to do with.
    ///
    /// [`Untracked`]: ZonalShiftRegistration::Untracked
    #[must_use]
    pub fn withdrawn(self) -> Self {
        match self {
            Self::Managed { arn, .. } | Self::Withdrawn { arn } => Self::Withdrawn { arn },
            Self::Untracked => Self::Untracked,
        }
    }
}

impl ElbState {
    /// Whether the load balancer `load_balancer_arn` is a managed resource of ARC, and the
    /// zones it is deployed in, as this store holds it NOW.
    ///
    /// Derived, never accumulated, so the integration turned off and the zone set changed
    /// are the same answer. An identifier this store holds NO load balancer for is
    /// [`ZonalShiftRegistration::Untracked`] rather than a withdrawal of it: the only ARNs
    /// this service publishes to the shared registry are the ones it minted itself and
    /// still holds, never a string a request carried.
    pub fn zonal_shift_registration(&self, load_balancer_arn: &str) -> ZonalShiftRegistration {
        let Some(load_balancer) = self.load_balancers.get(load_balancer_arn) else {
            return ZonalShiftRegistration::Untracked;
        };
        if !load_balancer.zonal_shift_enabled() {
            return ZonalShiftRegistration::Withdrawn {
                arn: load_balancer.arn.clone(),
            };
        }
        ZonalShiftRegistration::Managed {
            arn: load_balancer.arn.clone(),
            name: load_balancer.name.clone(),
            zones: load_balancer
                .availability_zones
                .iter()
                .map(|zone| ZonePlacement {
                    subnet_id: zone.subnet_id.clone(),
                    zone_name: zone.zone_name.clone(),
                })
                .collect(),
        }
    }
}

impl LoadBalancerRecord {
    /// Whether zonal-shift integration is on for this load balancer: the value its
    /// attribute collection carries [`ZONAL_SHIFT_ATTRIBUTE`] at, read through the catalog
    /// so the documented default and the families that carry the attribute at all are
    /// decided in one place.
    pub fn zonal_shift_enabled(&self) -> bool {
        let Some(spec) = super::load_balancer_attribute(ZONAL_SHIFT_ATTRIBUTE) else {
            return false;
        };
        if !spec.applies_to(self) {
            return false;
        }
        self.attributes
            .get(ZONAL_SHIFT_ATTRIBUTE)
            .cloned()
            .or_else(|| spec.default_for(self))
            .as_deref()
            == Some("true")
    }
}

/// Record in the shared registry exactly what the load balancer `registration` describes
/// is to ARC.
///
/// Called by every operation that can change it — the attribute write that turns the
/// integration on or off, the subnet write that changes which zones the load balancer
/// spans, and the delete that removes it — with the registration already read off this
/// service's store and that store's lock RELEASED, because the zone IDs are read from EC2
/// in between. The entry is REPLACED, so a zone the store no longer puts the load balancer
/// in cannot survive the call, and a load balancer that is not a managed resource has no
/// entry at all.
///
/// A registration of [`ZonalShiftRegistration::Untracked`] writes NOTHING: the registry is
/// shared with every other service of the process, and the only identifiers this service
/// may name in it are the load balancer ARNs it minted and holds itself — never one a
/// request supplied, which could otherwise name another resource's entry.
pub async fn record_zonal_shift_registration<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    registration: &ZonalShiftRegistration,
) {
    let Some(registry) = managed_resource_registry(engine) else {
        return;
    };
    match registration {
        ZonalShiftRegistration::Untracked => {}
        ZonalShiftRegistration::Withdrawn { arn } => registry.lock().deregister(arn),
        ZonalShiftRegistration::Managed { arn, name, zones } => {
            let availability_zones = zone_identifiers(ctx, zones).await;
            registry.lock().register(arn, name, availability_zones);
        }
    }
}

/// How ARC names each zone the load balancer is deployed in: the Availability Zone ID EC2
/// reports for the subnet that put it there, and the zone NAME this store holds for a
/// subnet EC2 reports no ID for — EC2 owns the pairing, so nothing is derived here.
async fn zone_identifiers(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    zones: &[ZonePlacement],
) -> Vec<String> {
    let subnet_ids: Vec<String> = zones.iter().map(|zone| zone.subnet_id.clone()).collect();
    let zone_ids = super::network::zone_ids_of_subnets(ctx, &super::request_region(ctx), &subnet_ids)
        .await;
    zones
        .iter()
        .map(|zone| {
            zone_ids
                .get(&zone.subnet_id)
                .cloned()
                .unwrap_or_else(|| zone.zone_name.clone())
        })
        .collect()
}
