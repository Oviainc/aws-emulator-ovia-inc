// SPDX-License-Identifier: CC0-1.0

//! The MANAGED RESOURCE REGISTRY of Amazon Application Recovery Controller: which
//! resources of the other services are registered with ARC for zonal shift.
//!
//! ARC models no operation that registers one — "Resources are automatically registered
//! with ARC by Amazon Web Services services" (the `StartZonalShift` documentation of the
//! ARC zonal shift model) — so a resource becomes managed through the service that OWNS
//! it: turning zonal-shift integration on for a load balancer is a call to Elastic Load
//! Balancing, and ARC then reports that load balancer. ARC cannot see that service's
//! store, and the owning service cannot see ARC's, so the fact lives here: ONE registry in
//! the shared engine, written by the owner on every call that turns the integration on or
//! off, changes which Availability Zones the resource spans, or removes it, and read by ARC
//! wherever it answers about a managed resource.
//!
//! An entry carries only what ARC reports about a resource it did not create: the ARN a
//! caller names it by, its name, and the Availability Zones it is deployed in — the key set
//! of `appliedWeights`. Everything else ARC answers (the shifts in progress, the weights
//! they move, a practice run configuration) is ARC's own and stays in ARC's store.
//!
//! The registry is BOUNDED in both dimensions a caller can grow — how many resources it
//! holds ([`MAX_REGISTERED_RESOURCES`]) and how many zones one resource spans
//! ([`MAX_ZONES_PER_RESOURCE`]) — so no sequence of calls grows it without end: past the
//! resource ceiling the OLDEST registration is dropped, which keeps the resource a running
//! workflow just registered reportable.
//!
//! LOCK ORDER: a service takes its OWN store first and this registry second, and nothing
//! holds this registry while taking another store — so two services cannot invert the
//! order between them.

use std::collections::{BTreeMap, VecDeque};

/// How many registered resources the registry holds at once.
///
/// An EMULATOR ceiling, not an AWS quota: the registry lives in memory for the lifetime of
/// the process, so without a bound a caller registering resources in a loop would grow it
/// without end. It is set far above any ordinary workflow, and past it the OLDEST
/// registration is dropped — never a newer one — so a resource a running workflow just
/// registered is always still reportable.
pub const MAX_REGISTERED_RESOURCES: usize = 10_000;

/// How many Availability Zones ONE registered resource is recorded as spanning.
///
/// An EMULATOR ceiling and the companion of [`MAX_REGISTERED_RESOURCES`]: a resource spans
/// the zones its subnets put it in, and a caller naming many subnets would otherwise grow
/// one entry without end. It is set far above the number of zones any published Region has.
pub const MAX_ZONES_PER_RESOURCE: usize = 64;

/// One resource an Amazon Web Services service has registered with ARC for zonal shift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredResource {
    /// The resource's ARN, which is the `resourceIdentifier` a caller names it by.
    pub arn: String,
    /// The resource's name, as the service that owns it holds it.
    pub name: String,
    /// The Availability Zones the resource is deployed in, in the order the owner
    /// recorded them and without repeats: the key set of ARC's `appliedWeights`.
    pub availability_zones: Vec<String>,
}

/// Which resources are registered with ARC for zonal shift.
///
/// Registered in the shared engine under THIS type, so the service that owns a resource
/// and ARC, which reports it, read one registry instead of each keeping its own copy.
#[derive(Debug, Default)]
pub struct ManagedResourceRegistry {
    /// The registered resources, keyed by ARN.
    by_arn: BTreeMap<String, RegisteredResource>,
    /// Registration order, so the ceiling drops the oldest resource first.
    order: VecDeque<String>,
}

impl ManagedResourceRegistry {
    /// Record that the resource `arn` is registered with ARC, deployed in
    /// `availability_zones` and named `name`.
    ///
    /// This REPLACES the whole entry, so an owner whose resource changed — its zones, its
    /// name — records what it holds NOW and nothing else, and a repeated registration of
    /// an unchanged resource changes nothing. Blank zone entries and repeats are dropped,
    /// the zone list is held to [`MAX_ZONES_PER_RESOURCE`], and a registration that takes
    /// the registry past [`MAX_REGISTERED_RESOURCES`] drops the oldest one.
    pub fn register<I, S>(&mut self, arn: &str, name: &str, availability_zones: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let arn = arn.trim();
        if arn.is_empty() {
            return;
        }
        // Taken from the iterator under the ceiling, so one call cannot collect an
        // unbounded number of zones into memory on its way to being bounded.
        let mut zones: Vec<String> = Vec::new();
        for zone in availability_zones {
            let zone = zone.as_ref().trim().to_string();
            if zone.is_empty() || zones.contains(&zone) {
                continue;
            }
            zones.push(zone);
            if zones.len() == MAX_ZONES_PER_RESOURCE {
                break;
            }
        }
        let fresh = !self.by_arn.contains_key(arn);
        self.by_arn.insert(
            arn.to_string(),
            RegisteredResource {
                arn: arn.to_string(),
                name: name.trim().to_string(),
                availability_zones: zones,
            },
        );
        if fresh {
            self.order.push_back(arn.to_string());
            while self.order.len() > MAX_REGISTERED_RESOURCES {
                if let Some(oldest) = self.order.pop_front() {
                    self.by_arn.remove(&oldest);
                }
            }
        }
    }

    /// Record that the resource `arn` is no longer registered with ARC: its integration
    /// was turned off, or the resource itself is gone.
    ///
    /// A no-op for a resource the registry holds nothing for, so a repeated call — a
    /// delete of a resource that never had the integration on — is harmless.
    pub fn deregister(&mut self, arn: &str) {
        let arn = arn.trim();
        self.by_arn.remove(arn);
        self.order.retain(|registered| registered != arn);
    }

    /// The registration of one resource, or `None` for an ARN no service has registered.
    #[must_use]
    pub fn get(&self, arn: &str) -> Option<&RegisteredResource> {
        self.by_arn.get(arn.trim())
    }

    /// Every registered resource, in ARN order so two reads report the same sequence.
    #[must_use]
    pub fn resources(&self) -> Vec<RegisteredResource> {
        self.by_arn.values().cloned().collect()
    }

    /// How many resources are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_arn.len()
    }

    /// Whether no resource is registered at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_arn.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A load balancer ARN, numbered.
    fn load_balancer(index: usize) -> String {
        format!(
            "arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/net/lb-{index:06}\
             /0123456789abcdef"
        )
    }

    #[test]
    fn a_registered_resource_is_reported_with_the_zones_it_spans() {
        let mut registry = ManagedResourceRegistry::default();
        // The rows differ along how many zones a resource spans, and whether the owner
        // named a zone twice or named a blank one.
        let rows: &[(&str, &[&str], &[&str])] = &[
            ("one zone", &["usw2-az1"], &["usw2-az1"]),
            (
                "three zones",
                &["usw2-az1", "usw2-az2", "usw2-az3"],
                &["usw2-az1", "usw2-az2", "usw2-az3"],
            ),
            (
                "the same zone twice",
                &["usw2-az1", "usw2-az1"],
                &["usw2-az1"],
            ),
            (
                "a blank entry beside a zone",
                &["", "   ", "usw2-az4"],
                &["usw2-az4"],
            ),
        ];
        for (index, (case, recorded, expected)) in rows.iter().enumerate() {
            let arn = load_balancer(index);
            registry.register(&arn, "lb", recorded.iter());
            let entry = registry
                .get(&arn)
                .unwrap_or_else(|| panic!("case {case}: expected the resource registered"));
            assert_eq!(
                entry.availability_zones,
                expected.iter().map(|zone| zone.to_string()).collect::<Vec<_>>(),
                "case {case}: expected the zones it spans, got {:?}",
                entry.availability_zones
            );
        }
        assert_eq!(
            registry.len(),
            rows.len(),
            "case every row registered: expected {} resources, got {}",
            rows.len(),
            registry.len()
        );
    }

    #[test]
    fn a_re_registration_replaces_the_whole_entry() {
        let mut registry = ManagedResourceRegistry::default();
        let arn = load_balancer(0);
        registry.register(&arn, "before", ["usw2-az1", "usw2-az2"]);
        registry.register(&arn, "after", ["usw2-az3"]);
        let entry = registry.get(&arn).expect("the resource is registered");
        assert_eq!(
            (entry.name.as_str(), entry.availability_zones.as_slice()),
            ("after", ["usw2-az3".to_string()].as_slice()),
            "case a re-registration: expected only what was recorded last, got {entry:?}"
        );
        assert_eq!(
            registry.len(),
            1,
            "case a re-registration: expected one resource, got {}",
            registry.len()
        );
    }

    #[test]
    fn a_deregistered_resource_is_reported_by_nothing() {
        let mut registry = ManagedResourceRegistry::default();
        registry.register(&load_balancer(0), "kept", ["usw2-az1"]);
        registry.register(&load_balancer(1), "dropped", ["usw2-az2"]);
        registry.deregister(&load_balancer(1));
        // Deregistering twice, and deregistering a resource that was never registered,
        // each change nothing.
        registry.deregister(&load_balancer(1));
        registry.deregister(&load_balancer(7));
        assert_eq!(
            registry.resources(),
            vec![RegisteredResource {
                arn: load_balancer(0),
                name: "kept".to_string(),
                availability_zones: vec!["usw2-az1".to_string()],
            }],
            "case a deregistered resource: expected only the kept one, got {:?}",
            registry.resources()
        );
        registry.deregister(&load_balancer(0));
        assert!(
            registry.is_empty(),
            "case every resource deregistered: expected an empty registry, got {:?}",
            registry.resources()
        );
    }

    #[test]
    fn the_ceiling_drops_the_oldest_registration_first() {
        let mut registry = ManagedResourceRegistry::default();
        for index in 0..MAX_REGISTERED_RESOURCES {
            registry.register(&load_balancer(index), "lb", ["usw2-az1"]);
        }
        // A re-registration is not a new resource, so it moves nothing out.
        registry.register(&load_balancer(0), "lb", ["usw2-az2"]);
        assert_eq!(
            registry.len(),
            MAX_REGISTERED_RESOURCES,
            "case a registry filled to the ceiling: expected {MAX_REGISTERED_RESOURCES} \
             resources, got {}",
            registry.len()
        );

        registry.register(&load_balancer(MAX_REGISTERED_RESOURCES), "lb", ["usw2-az1"]);
        assert_eq!(
            registry.len(),
            MAX_REGISTERED_RESOURCES,
            "case one past the ceiling: expected the ceiling held, got {}",
            registry.len()
        );
        assert!(
            registry.get(&load_balancer(0)).is_none(),
            "case one past the ceiling: expected the oldest registration dropped"
        );
        assert!(
            registry.get(&load_balancer(1)).is_some()
                && registry
                    .get(&load_balancer(MAX_REGISTERED_RESOURCES))
                    .is_some(),
            "case one past the ceiling: expected the newest registrations kept"
        );
    }

    #[test]
    fn one_resource_cannot_span_more_zones_than_the_per_resource_ceiling() {
        let mut registry = ManagedResourceRegistry::default();
        let arn = load_balancer(0);
        let many: Vec<String> = (0..MAX_ZONES_PER_RESOURCE * 4)
            .map(|index| format!("usw2-az{index}"))
            .collect();
        registry.register(&arn, "lb", &many);
        let entry = registry.get(&arn).expect("the resource is registered");
        assert_eq!(
            entry.availability_zones.len(),
            MAX_ZONES_PER_RESOURCE,
            "case a registration naming {} zones: expected the ceiling held, got {}",
            many.len(),
            entry.availability_zones.len()
        );
    }

    #[test]
    fn an_arn_nothing_registered_is_reported_by_nothing() {
        let registry = ManagedResourceRegistry::default();
        assert!(
            registry.get(&load_balancer(0)).is_none() && registry.is_empty(),
            "case an empty registry: expected no registration, got {:?}",
            registry.resources()
        );
    }
}
