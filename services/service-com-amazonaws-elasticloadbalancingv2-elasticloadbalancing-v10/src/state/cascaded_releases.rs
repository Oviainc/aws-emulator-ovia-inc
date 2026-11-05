// SPDX-License-Identifier: CC0-1.0

//! What this store released WITH ITS CONTAINER, which is what makes a delete of it
//! IDEMPOTENT.
//!
//! Elastic Load Balancing releases a resource together with the one that contains it:
//! deleting a load balancer "also deletes its listeners" (`DeleteLoadBalancer`), and a
//! listener's rules go with the listener. So a client tearing a deployment down — or a
//! template delete walking its resources — asks to delete a listener or a rule the
//! cascade has ALREADY deleted, and the deletion it asks for has been performed: the call
//! completes rather than reporting a resource the caller is right to believe is gone.
//!
//! A resource a caller deleted ITSELF is not recorded here, and neither is an identifier
//! this store never held: a second `DeleteRule` on a rule the caller removed, and a delete
//! of an ARN nothing ever carried, are the modelled `NotFound` of the operation.
//!
//! The record is a BOUNDED window ([`CascadedReleases::CAPACITY`]), the emulator's own
//! bound as every quota in this module is: it holds far more cascades than any workflow
//! tears down, and past it the oldest is forgotten rather than the store growing without
//! end — a delete of a cascade that old is the modelled `NotFound` again.

use std::collections::{HashSet, VecDeque};

/// The ARNs this store released with their container, newest last.
#[derive(Debug, Default)]
pub struct CascadedReleases {
    /// The order the ARNs were released in, so the oldest is what the bound drops.
    order: VecDeque<String>,
    /// The same ARNs as a set, so a delete resolves one in constant time.
    held: HashSet<String>,
}

impl CascadedReleases {
    /// How many cascaded releases this window remembers.
    pub const CAPACITY: usize = 10_000;

    /// Record that `arn` was released with the resource that contained it.
    pub fn record(&mut self, arn: &str) {
        if !self.held.insert(arn.to_string()) {
            return;
        }
        self.order.push_back(arn.to_string());
        while self.order.len() > Self::CAPACITY {
            if let Some(dropped) = self.order.pop_front() {
                self.held.remove(&dropped);
            }
        }
    }

    /// Whether `arn` names a resource this store released with its container, so a
    /// delete of it asks for something already performed.
    pub fn holds(&self, arn: &str) -> bool {
        self.held.contains(arn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recorded_release_is_held_and_an_unrecorded_one_is_not() {
        let mut releases = CascadedReleases::default();
        assert!(
            !releases.holds("arn:aws:elasticloadbalancing:us-east-1:1:listener/app/a/1/2"),
            "case nothing recorded: no ARN is held"
        );
        releases.record("arn:aws:elasticloadbalancing:us-east-1:1:listener/app/a/1/2");
        assert!(
            releases.holds("arn:aws:elasticloadbalancing:us-east-1:1:listener/app/a/1/2"),
            "case one recorded: that ARN is held"
        );
        assert!(
            !releases.holds("arn:aws:elasticloadbalancing:us-east-1:1:listener/app/a/1/3"),
            "case one recorded: another ARN is not"
        );
    }

    #[test]
    fn the_window_forgets_the_oldest_release_past_its_bound() {
        let mut releases = CascadedReleases::default();
        for index in 0..=CascadedReleases::CAPACITY {
            releases.record(&format!("arn-{index}"));
        }
        assert!(
            !releases.holds("arn-0"),
            "case past the bound: the oldest release is forgotten"
        );
        assert!(
            releases.holds(&format!("arn-{}", CascadedReleases::CAPACITY)),
            "case past the bound: the newest release is held"
        );
        // Recording the same ARN twice is one entry, so a repeat cannot push the window.
        let before = releases.order.len();
        releases.record(&format!("arn-{}", CascadedReleases::CAPACITY));
        assert_eq!(
            releases.order.len(),
            before,
            "case a repeated release: it is held once, got {} entries",
            releases.order.len()
        );
    }
}
