// SPDX-License-Identifier: CC0-1.0

//! The CERTIFICATE ASSOCIATION DIRECTORY: which resources of the other services
//! currently reference an ACM certificate.
//!
//! A certificate is held by ACM but SERVED by another service: an API Gateway custom
//! domain name presents one, an Elastic Load Balancing listener presents one, a
//! distribution presents one. ACM cannot see those resources — each lives in the store
//! of the service that owns it — yet several of its own answers are about them: a
//! certificate something still references may not be deleted or revoked, and a
//! certificate's report names the resources using it.
//!
//! This is that record: ONE directory in the shared engine, written by the service that
//! owns the REFERENCING resource (on the call that creates it, on every call that
//! changes which certificates it references, and on the call that removes it) and read by
//! ACM. Nothing here names a wire shape — the directory holds ARNs on both sides, and a
//! reader projects them onto the shapes its own model declares.
//!
//! The directory is keyed by the REFERENCING resource, because that is the side whose
//! owner writes it: a resource's whole certificate set is replaced in one call
//! ([`CertificateAssociationDirectory::record_resource`]), so a reference a resource no
//! longer holds cannot survive an update, and a reader asks the question from the other
//! side ([`CertificateAssociationDirectory::resources_using`]).
//!
//! GIVING UP A REFERENCE TAKES TIME. A resource stops presenting a certificate only once
//! the work that takes it away has actually run — the endpoints it was propagated to have
//! to be torn down or migrated — and the owning service ACCEPTS that work rather than
//! completing it inside the call. So a reference the owner gives up is recorded as
//! RELEASING at the instant the release falls due, and counts as in use until then; the
//! instant is stored and nothing sleeps for it, exactly as every other transition the
//! clock owes ([`CertificateAssociationDirectory::materialize_due`]).
//!
//! The directory is BOUNDED in every dimension a caller can grow — how many resources
//! it holds associations for ([`MAX_ASSOCIATED_RESOURCES`]), how many certificates one
//! resource can hold and be giving up ([`MAX_CERTIFICATES_PER_RESOURCE`]), and how long
//! a release can stay owed ([`MAX_RELEASE_DELAY`]) — so no sequence of calls grows it
//! without end: the oldest resource's associations are dropped first, which keeps the
//! resources a live workflow just created resolvable, and every write first applies the
//! releases the clock has already made due.
//!
//! LOCK ORDER: a service takes its OWN store first and this directory second, and nothing
//! holds this directory while taking another store — so two services cannot invert the
//! order between them.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, SystemTime};

/// How many referencing resources the directory holds associations for at once.
///
/// An EMULATOR ceiling, not an AWS quota: this directory lives in memory for the
/// lifetime of the process, so without a bound a caller creating resources in a loop
/// would grow it without end. It is set far above any ordinary workflow, and past it the
/// OLDEST resource's associations are dropped — never a newer one's — so a resource a
/// running workflow just created is always still accounted for.
pub const MAX_ASSOCIATED_RESOURCES: usize = 10_000;

/// How many certificates ONE resource's entry accounts for — counted over the ones it
/// references now and the ones it is still giving up together.
///
/// An EMULATOR ceiling, not an AWS quota, and the companion of
/// [`MAX_ASSOCIATED_RESOURCES`]: without it a caller could grow one entry without end by
/// pointing the same resource at fresh certificates over and over. It is set far above
/// any real resource's reference count (the largest published list of certificates on one
/// AWS resource is a load balancer listener's, in the tens; a custom domain name names at
/// most a couple). Past it, the ARNs a single call names beyond the ceiling are not
/// accounted for, and a reference being given up is dropped EARLIEST-DUE first — the one
/// closest to being released anyway.
pub const MAX_CERTIFICATES_PER_RESOURCE: usize = 64;

/// The longest a reference the owner has given up stays accounted for.
///
/// An EMULATOR ceiling: a release is owed a span after the call that starts it, and that
/// span belongs to the service that owns the resource — but no span may pin a given-up
/// reference for the life of the process, so one longer than this is clamped to it. Set
/// far above any teardown or migration span a service states.
pub const MAX_RELEASE_DELAY: Duration = Duration::from_secs(300);

/// What one referencing resource is using: the certificates it references now, and the
/// ones it has given up but is still presenting until the release falls due.
#[derive(Debug, Default)]
struct ResourceAssociations {
    /// The certificate ARNs the resource references now.
    held: BTreeSet<String>,
    /// The certificate ARNs it has given up, each with the instant its release falls
    /// due. Until that instant the resource is still using the certificate.
    releasing: BTreeMap<String, SystemTime>,
}

impl ResourceAssociations {
    /// Whether this resource is using `certificate_arn`: it references it now, or it has
    /// given it up and the release is not due yet.
    fn uses(&self, certificate_arn: &str) -> bool {
        self.held.contains(certificate_arn) || self.releasing.contains_key(certificate_arn)
    }

    /// Every certificate this resource is using.
    fn in_use(&self) -> impl Iterator<Item = &String> {
        self.held.iter().chain(self.releasing.keys())
    }

    /// Whether the resource is using nothing at all, so its entry carries no fact.
    fn is_idle(&self) -> bool {
        self.held.is_empty() && self.releasing.is_empty()
    }

    /// Hold this entry to [`MAX_CERTIFICATES_PER_RESOURCE`] certificates, dropping the
    /// references being given up EARLIEST-DUE first — each of those is already on its way
    /// out, so dropping one releases a certificate a little early rather than holding a
    /// certificate the resource still presents.
    fn enforce_ceiling(&mut self) {
        while self.held.len() + self.releasing.len() > MAX_CERTIFICATES_PER_RESOURCE {
            let earliest = self
                .releasing
                .iter()
                .min_by_key(|(_, due_at)| **due_at)
                .map(|(certificate_arn, _)| certificate_arn.clone());
            match earliest {
                Some(certificate_arn) => {
                    self.releasing.remove(&certificate_arn);
                }
                // Nothing is being given up, so the held set is what is over the
                // ceiling: it was truncated as it was collected, and there is nothing
                // left to drop.
                None => break,
            }
        }
    }
}

/// Which certificates each referencing resource holds a reference to.
///
/// Registered in the shared engine under THIS type, so the service that associates a
/// certificate and ACM, which reports and protects it, read one directory instead of
/// each keeping its own copy.
#[derive(Debug, Default)]
pub struct CertificateAssociationDirectory {
    /// What each referencing resource ARN is using. A resource using nothing at all has
    /// no entry.
    by_resource: BTreeMap<String, ResourceAssociations>,
    /// Registration order, so the ceiling drops the oldest resource first.
    order: VecDeque<String>,
}

impl CertificateAssociationDirectory {
    /// Apply every release the clock has already made due, so what follows reads the
    /// associations as they stand NOW.
    ///
    /// This materialises what was owed at the same instant whether or not anybody
    /// called: a reference whose release has fallen due is gone, and a resource left
    /// using nothing drops out of the directory altogether. Every READER calls it first;
    /// a writer applies what the entry it writes owed, so the sweep is never on the write
    /// path of a caller creating resources in a loop.
    pub fn materialize_due(&mut self, now: SystemTime) {
        let by_resource = &mut self.by_resource;
        by_resource.retain(|_, associations| {
            associations.releasing.retain(|_, due_at| *due_at > now);
            !associations.is_idle()
        });
        // The eviction order holds the same resources the records do.
        self.order
            .retain(|resource_arn| by_resource.contains_key(resource_arn));
    }

    /// Record that `resource_arn` now references exactly `certificate_arns`, giving up
    /// every other reference it held — each of which stays in use until `settles_in` after
    /// `now`, the span the work that takes it away takes to complete.
    ///
    /// This REPLACES what the resource referenced before, so the owner of a resource
    /// whose certificates changed records the new set and nothing else. Blank entries are
    /// ignored, the releases this resource's own entry already owed are applied first,
    /// and the entry is held to [`MAX_CERTIFICATES_PER_RESOURCE`] certificates with
    /// `settles_in` clamped to [`MAX_RELEASE_DELAY`], so no sequence of calls can grow
    /// one entry without end.
    pub fn record_resource<I, S>(
        &mut self,
        resource_arn: &str,
        certificate_arns: I,
        now: SystemTime,
        settles_in: Duration,
    ) where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let release_due_at = now + settles_in.min(MAX_RELEASE_DELAY);
        // Taken from the iterator under the ceiling, so one call cannot collect an
        // unbounded number of ARNs into memory on its way to being bounded.
        let held: BTreeSet<String> = certificate_arns
            .into_iter()
            .map(|arn| arn.as_ref().trim().to_string())
            .filter(|arn| !arn.is_empty())
            .take(MAX_CERTIFICATES_PER_RESOURCE)
            .collect();
        let resource_arn = resource_arn.trim();
        let fresh = !self.by_resource.contains_key(resource_arn);
        let associations = self.by_resource.entry(resource_arn.to_string()).or_default();
        // What this entry already owed: applied here, so a resource written over and
        // over cannot accumulate releases that have long since fallen due. The sweep
        // over every entry is [`Self::materialize_due`], which every reader calls.
        associations.releasing.retain(|_, due_at| *due_at > now);
        for given_up in associations.held.difference(&held) {
            associations
                .releasing
                .insert(given_up.clone(), release_due_at);
        }
        // A certificate the resource references again is not being given up.
        associations
            .releasing
            .retain(|certificate_arn, _| !held.contains(certificate_arn));
        associations.held = held;
        associations.enforce_ceiling();
        if associations.is_idle() {
            self.by_resource.remove(resource_arn);
            self.order.retain(|arn| arn != resource_arn);
            return;
        }
        if fresh {
            self.order.push_back(resource_arn.to_string());
            while self.order.len() > MAX_ASSOCIATED_RESOURCES {
                if let Some(oldest) = self.order.pop_front() {
                    self.by_resource.remove(&oldest);
                }
            }
        }
    }

    /// Record that `resource_arn` is being removed: it gives up every reference it
    /// holds, each staying in use until `settles_in` after `now` — the span the removal
    /// the owning service accepted takes to complete.
    ///
    /// A no-op for a resource the directory holds nothing for, so a repeated delete is
    /// harmless.
    pub fn release_resource(&mut self, resource_arn: &str, now: SystemTime, settles_in: Duration) {
        self.record_resource(resource_arn, ::std::iter::empty::<&str>(), now, settles_in);
    }

    /// The ARNs of the resources using `certificate_arn`, in ARN order so two reads of
    /// one certificate report the same list.
    pub fn resources_using(&self, certificate_arn: &str) -> Vec<String> {
        self.by_resource
            .iter()
            .filter(|(_, associations)| associations.uses(certificate_arn))
            .map(|(resource_arn, _)| resource_arn.clone())
            .collect()
    }

    /// Whether any resource is using `certificate_arn`.
    pub fn is_in_use(&self, certificate_arn: &str) -> bool {
        self.by_resource
            .values()
            .any(|associations| associations.uses(certificate_arn))
    }

    /// Every certificate ARN some resource is using, as one set.
    ///
    /// For a reader that has to answer "is this one in use" for MANY certificates at
    /// once — a listing, a search that filters or sorts on it — so it takes the
    /// directory's answer once rather than asking it per certificate.
    pub fn certificates_in_use(&self) -> BTreeSet<String> {
        self.by_resource
            .values()
            .flat_map(ResourceAssociations::in_use)
            .cloned()
            .collect()
    }

    /// How many referencing resources the directory holds associations for.
    pub fn len(&self) -> usize {
        self.by_resource.len()
    }

    /// Whether the directory holds no association at all.
    pub fn is_empty(&self) -> bool {
        self.by_resource.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// An ACM certificate ARN, numbered.
    fn certificate(index: usize) -> String {
        format!("arn:aws:acm:us-west-2:000000000000:certificate/cert-{index:06}")
    }

    /// The ARN of a resource that references a certificate, numbered.
    fn resource(index: usize) -> String {
        format!("arn:aws:apigateway:us-west-2::/domainnames/host-{index:06}.example.com")
    }

    /// An instant every test records against, so nothing falls due mid-test on the wall
    /// clock.
    fn at() -> SystemTime {
        SystemTime::now()
    }

    /// A release span long enough that no test sees it fall due on its own, and inside
    /// [`MAX_RELEASE_DELAY`] so it is recorded as given.
    const SETTLES_IN: Duration = Duration::from_secs(60);

    #[test]
    fn a_recorded_resource_is_reported_from_the_certificate_side() {
        let mut directory = CertificateAssociationDirectory::default();
        directory.record_resource(
            &resource(1),
            [certificate(1), certificate(2)],
            at(),
            SETTLES_IN,
        );
        directory.record_resource(&resource(0), [certificate(1)], at(), SETTLES_IN);

        assert_eq!(
            directory.resources_using(&certificate(1)),
            vec![resource(0), resource(1)],
            "case two resources on one certificate: expected both in ARN order, got {:?}",
            directory.resources_using(&certificate(1))
        );
        assert_eq!(
            directory.resources_using(&certificate(2)),
            vec![resource(1)],
            "case one resource on one certificate: expected just it, got {:?}",
            directory.resources_using(&certificate(2))
        );
        assert!(
            directory.resources_using(&certificate(9)).is_empty()
                && !directory.is_in_use(&certificate(9)),
            "case a certificate nothing references: expected no resource, got {:?}",
            directory.resources_using(&certificate(9))
        );
        assert_eq!(
            directory.certificates_in_use(),
            BTreeSet::from([certificate(1), certificate(2)]),
            "case the whole set: expected every referenced certificate once, got {:?}",
            directory.certificates_in_use()
        );
    }

    #[test]
    fn a_rotated_reference_stays_in_use_until_its_release_falls_due() {
        let mut directory = CertificateAssociationDirectory::default();
        directory.record_resource(&resource(0), [certificate(0)], at(), SETTLES_IN);

        // The resource is pointed at another certificate, giving the first one up — but
        // it is still presenting it until the release falls due.
        let recorded_at = at();
        directory.record_resource(&resource(0), [certificate(1)], recorded_at, SETTLES_IN);
        assert_eq!(
            directory.resources_using(&certificate(0)),
            vec![resource(0)],
            "case a release not yet due: expected the certificate still in use, got {:?}",
            directory.resources_using(&certificate(0))
        );
        assert_eq!(
            directory.resources_using(&certificate(1)),
            vec![resource(0)],
            "case a release not yet due: expected the new certificate in use too, got {:?}",
            directory.resources_using(&certificate(1))
        );

        // Once it is due, the given-up reference is gone and the new one stands.
        directory.materialize_due(recorded_at + SETTLES_IN + Duration::from_secs(1));
        assert!(
            !directory.is_in_use(&certificate(0)),
            "case a release that fell due: expected the given-up certificate free, got {:?}",
            directory.resources_using(&certificate(0))
        );
        assert_eq!(
            directory.certificates_in_use(),
            BTreeSet::from([certificate(1)]),
            "case a release that fell due: expected only the current reference, got {:?}",
            directory.certificates_in_use()
        );
        assert_eq!(
            directory.len(),
            1,
            "case a release that fell due: expected the resource still on record, got {}",
            directory.len()
        );
    }

    #[test]
    fn a_reference_taken_up_again_is_no_longer_being_given_up() {
        let mut directory = CertificateAssociationDirectory::default();
        let recorded_at = at();
        directory.record_resource(&resource(0), [certificate(0)], recorded_at, SETTLES_IN);
        directory.record_resource(&resource(0), [certificate(1)], recorded_at, SETTLES_IN);
        directory.record_resource(&resource(0), [certificate(0)], recorded_at, SETTLES_IN);

        // The certificate the resource came back to is held, not released, so the
        // instant that release would have fallen due changes nothing.
        directory.materialize_due(recorded_at + SETTLES_IN + Duration::from_secs(1));
        assert_eq!(
            directory.certificates_in_use(),
            BTreeSet::from([certificate(0)]),
            "case a reference taken up again: expected it still in use, got {:?}",
            directory.certificates_in_use()
        );
    }

    #[test]
    fn a_resource_that_references_nothing_holds_no_entry() {
        let mut directory = CertificateAssociationDirectory::default();
        // Each row is a set an owner records that names no certificate at all, for a
        // resource that was holding no reference either.
        let rows: &[(&str, &[&str])] = &[
            ("an empty set", &[]),
            ("a blank entry", &[""]),
            ("a whitespace entry", &["   "]),
        ];
        for (case, certificates) in rows {
            directory.record_resource(&resource(0), certificates.iter(), at(), SETTLES_IN);
            assert!(
                directory.is_empty(),
                "case {case}: expected no association on record, got {} resource(s)",
                directory.len()
            );
        }
    }

    #[test]
    fn releasing_a_resource_frees_only_its_certificates_and_only_when_due() {
        let mut directory = CertificateAssociationDirectory::default();
        directory.record_resource(&resource(0), [certificate(0)], at(), SETTLES_IN);
        directory.record_resource(
            &resource(1),
            [certificate(0), certificate(1)],
            at(),
            SETTLES_IN,
        );

        // The removal of one resource is ACCEPTED, so until it completes that resource
        // is still presenting what it held.
        let released_at = at();
        directory.release_resource(&resource(1), released_at, SETTLES_IN);
        assert_eq!(
            directory.resources_using(&certificate(1)),
            vec![resource(1)],
            "case a removal not yet complete: expected the certificate still in use, got {:?}",
            directory.resources_using(&certificate(1))
        );

        let settled = released_at + SETTLES_IN + Duration::from_secs(1);
        directory.materialize_due(settled);
        assert_eq!(
            directory.resources_using(&certificate(0)),
            vec![resource(0)],
            "case a completed removal: expected the other resource to keep its reference, got \
             {:?}",
            directory.resources_using(&certificate(0))
        );
        assert!(
            !directory.is_in_use(&certificate(1)),
            "case a completed removal: expected the certificate only it referenced to be free"
        );

        // Releasing a resource the directory holds nothing for changes nothing.
        directory.release_resource(&resource(1), settled, SETTLES_IN);
        directory.release_resource(&resource(7), settled, SETTLES_IN);
        assert_eq!(
            directory.len(),
            1,
            "case releasing an absent resource: expected the remaining record untouched, got {}",
            directory.len()
        );

        directory.release_resource(&resource(0), settled, SETTLES_IN);
        directory.materialize_due(settled + SETTLES_IN + Duration::from_secs(1));
        assert!(
            directory.is_empty() && !directory.is_in_use(&certificate(0)),
            "case every resource released: expected an empty directory, got {} resource(s)",
            directory.len()
        );
    }

    #[test]
    fn the_ceiling_drops_the_oldest_resource_first() {
        let mut directory = CertificateAssociationDirectory::default();
        for index in 0..MAX_ASSOCIATED_RESOURCES {
            directory.record_resource(&resource(index), [certificate(index)], at(), SETTLES_IN);
        }
        assert_eq!(
            directory.len(),
            MAX_ASSOCIATED_RESOURCES,
            "case a directory filled to the ceiling: expected {MAX_ASSOCIATED_RESOURCES} \
             resources, got {}",
            directory.len()
        );

        // A re-record is not a new resource, so it moves nothing out.
        directory.record_resource(&resource(0), [certificate(0)], at(), SETTLES_IN);
        assert!(
            directory.is_in_use(&certificate(0)) && directory.len() == MAX_ASSOCIATED_RESOURCES,
            "case a re-record at the ceiling: expected the resource kept, got {} resource(s)",
            directory.len()
        );

        // One more NEW resource evicts the oldest and only the oldest.
        directory.record_resource(
            &resource(MAX_ASSOCIATED_RESOURCES),
            [certificate(MAX_ASSOCIATED_RESOURCES)],
            at(),
            SETTLES_IN,
        );
        assert_eq!(
            directory.len(),
            MAX_ASSOCIATED_RESOURCES,
            "case one past the ceiling: expected the ceiling held, got {}",
            directory.len()
        );
        assert!(
            !directory.is_in_use(&certificate(0)),
            "case one past the ceiling: expected the oldest resource's associations dropped"
        );
        assert!(
            directory.is_in_use(&certificate(1))
                && directory.is_in_use(&certificate(MAX_ASSOCIATED_RESOURCES)),
            "case one past the ceiling: expected the newest associations kept"
        );
    }

    #[test]
    fn one_resource_cannot_grow_past_the_per_resource_ceiling() {
        let mut directory = CertificateAssociationDirectory::default();
        let recorded_at = at();

        // A single call naming more certificates than one resource may account for is
        // taken only up to the ceiling.
        let many: Vec<String> = (0..MAX_CERTIFICATES_PER_RESOURCE * 4)
            .map(certificate)
            .collect();
        directory.record_resource(&resource(0), &many, recorded_at, SETTLES_IN);
        assert_eq!(
            directory.certificates_in_use().len(),
            MAX_CERTIFICATES_PER_RESOURCE,
            "case one call naming {} certificates: expected the ceiling held, got {}",
            many.len(),
            directory.certificates_in_use().len()
        );

        // And so is a caller pointing the SAME resource at fresh certificates over and
        // over, which would otherwise pile up the references it keeps giving up.
        for round in 0..16 {
            let fresh: Vec<String> = (0..MAX_CERTIFICATES_PER_RESOURCE)
                .map(|index| certificate(1_000 + round * MAX_CERTIFICATES_PER_RESOURCE + index))
                .collect();
            directory.record_resource(&resource(0), &fresh, recorded_at, SETTLES_IN);
            assert!(
                directory.certificates_in_use().len() <= MAX_CERTIFICATES_PER_RESOURCE,
                "case round {round} of fresh certificates: expected at most \
                 {MAX_CERTIFICATES_PER_RESOURCE} accounted for, got {}",
                directory.certificates_in_use().len()
            );
            assert_eq!(
                directory.len(),
                1,
                "case round {round} of fresh certificates: expected one resource on record, got {}",
                directory.len()
            );
            // The set the resource references NOW is never what gets dropped.
            for held in &fresh {
                assert!(
                    directory.is_in_use(held),
                    "case round {round} of fresh certificates: expected the current reference \
                     {held} to be in use"
                );
            }
        }
    }

    #[test]
    fn a_release_span_is_clamped_to_the_directorys_horizon() {
        let mut directory = CertificateAssociationDirectory::default();
        let recorded_at = at();
        directory.record_resource(&resource(0), [certificate(0)], recorded_at, SETTLES_IN);
        // A span far past the horizon would otherwise pin the given-up reference for the
        // life of the process.
        directory.record_resource(
            &resource(0),
            [certificate(1)],
            recorded_at,
            Duration::from_secs(86_400),
        );

        directory.materialize_due(recorded_at + MAX_RELEASE_DELAY + Duration::from_secs(1));
        assert!(
            !directory.is_in_use(&certificate(0)),
            "case a span past the horizon: expected the release due by the horizon, got {:?}",
            directory.resources_using(&certificate(0))
        );
        assert!(
            directory.is_in_use(&certificate(1)),
            "case a span past the horizon: expected the current reference untouched"
        );
    }
}
