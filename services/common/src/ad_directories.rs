// SPDX-License-Identifier: CC0-1.0

//! The AD DIRECTORY INVENTORY: which Active Directory directories the account holds, so
//! the directory DATA plane can tell a directory of this account from one that is not.
//!
//! A directory is created by the AWS Directory Service CONTROL plane (`CreateDirectory`,
//! `CreateMicrosoftAD`) and deleted by it (`DeleteDirectory`); the Directory Service DATA
//! plane — which reads and writes the users, groups and memberships INSIDE a directory —
//! creates none and so cannot know from its own records whether a `DirectoryId` a request
//! names is one the account holds at all. It has to know, because every one of its
//! operations answers a directory the caller cannot reach by refusing the call rather than
//! by reporting anything about the directory.
//!
//! This is that record: ONE inventory in the shared engine, written by the control plane
//! when it creates a directory and cleared when it deletes one, read by the data plane.
//! Nothing here names a wire shape — the inventory holds directory ids — and nothing here
//! decides what a reader does with the answer: WHICH error a data-plane request gets for a
//! directory the inventory does not hold is that service's own.
//!
//! The inventory is a record of what this emulator CREATED, so a reader asking about an id
//! gets a yes or a no about this account's own directories and nothing else. It is BOUNDED
//! — see [`MAX_RECORDED_DIRECTORIES`], far above the control plane's own directory quota —
//! so nothing a caller loops on can grow it without end; the oldest registration is
//! dropped first, which keeps the directory a live workflow just created resolvable.
//!
//! LOCK ORDER: a service takes its OWN store first and this inventory second, and nothing
//! holds this inventory while taking another store — so two services cannot invert the
//! order between them.

use std::collections::{HashSet, VecDeque};

/// How many directories the inventory holds at once.
///
/// An EMULATOR ceiling, not an AWS quota: this inventory lives in memory for the lifetime
/// of the process, so without a bound a caller creating directories in a loop would grow
/// it without end. It sits far above the control plane's own per-account directory quota,
/// and past it the OLDEST registration is dropped — never a newer one — so a directory a
/// running workflow just created is always still resolvable.
pub const MAX_RECORDED_DIRECTORIES: usize = 1_000;

/// The inventory itself: every `DirectoryId` the control plane created and has not deleted.
///
/// Registered in the shared engine under THIS type, so the control plane that creates a
/// directory and the data plane that serves the objects inside it read one record instead
/// of each keeping its own copy.
#[derive(Debug, Default)]
pub struct AdDirectoryInventory {
    /// The ids held, so the question a reader asks — does the account hold this directory —
    /// is answered by one lookup.
    held: HashSet<String>,
    /// Registration order, so the ceiling drops the oldest entry first.
    order: VecDeque<String>,
}

impl AdDirectoryInventory {
    /// Record that the account holds the directory `directory_id`.
    ///
    /// Recording an id the inventory already holds changes nothing and does not move its
    /// place in the eviction order, so a control plane that re-registers a directory it
    /// already told the inventory about cannot age another one out.
    pub fn register(&mut self, directory_id: &str) {
        if self.held.insert(directory_id.to_string()) {
            self.order.push_back(directory_id.to_string());
            while self.order.len() > MAX_RECORDED_DIRECTORIES {
                if let Some(oldest) = self.order.pop_front() {
                    self.held.remove(&oldest);
                }
            }
        }
    }

    /// Drop the record of `directory_id`: the directory is gone, so the inventory no longer
    /// claims the account holds it.
    ///
    /// A no-op for an id the inventory holds no record of, so a repeated delete is harmless.
    pub fn forget(&mut self, directory_id: &str) {
        if self.held.remove(directory_id) {
            self.order.retain(|recorded| recorded != directory_id);
        }
    }

    /// Whether the account holds the directory `directory_id`.
    #[must_use]
    pub fn holds(&self, directory_id: &str) -> bool {
        self.held.contains(directory_id)
    }

    /// How many directories the inventory holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    /// Whether the inventory holds no directory at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{AdDirectoryInventory, MAX_RECORDED_DIRECTORIES};

    /// A `DirectoryId` of the form the models declare (`^d-[0-9a-f]{10}$`), one per rank.
    fn directory(rank: usize) -> String {
        format!("d-{rank:010x}")
    }

    /// RULE: the inventory answers for exactly the directories registered and not forgotten,
    /// whatever order the two happen in.
    #[test]
    fn the_inventory_holds_what_was_registered_and_not_forgotten() {
        let mut inventory = AdDirectoryInventory::default();
        inventory.register(&directory(1));
        inventory.register(&directory(2));
        // Registering twice is the same fact, not a second record.
        inventory.register(&directory(2));

        for (case, id, expected) in [
            ("a registered directory", directory(1), true),
            ("a directory registered twice", directory(2), true),
            ("a directory never registered", directory(3), false),
        ] {
            assert_eq!(
                inventory.holds(&id),
                expected,
                "case {case:?}: expected holds({id:?}) to be {expected}, got {}",
                inventory.holds(&id)
            );
        }
        assert_eq!(
            inventory.len(),
            2,
            "case two distinct directories: expected 2 records, got {}",
            inventory.len()
        );

        inventory.forget(&directory(1));
        // Forgetting an id the inventory never held changes nothing.
        inventory.forget(&directory(9));
        assert!(
            !inventory.holds(&directory(1)) && inventory.holds(&directory(2)),
            "case one directory forgotten: expected only the other to be held, got {} record(s)",
            inventory.len()
        );

        inventory.forget(&directory(2));
        assert!(
            inventory.is_empty(),
            "case every directory forgotten: expected an empty inventory, got {} record(s)",
            inventory.len()
        );
    }

    /// RULE: the inventory is bounded, and past the ceiling the OLDEST registration is the
    /// one dropped, so the directory just created is still resolvable.
    #[test]
    fn the_inventory_is_bounded_and_drops_its_oldest_registration() {
        let mut inventory = AdDirectoryInventory::default();
        for rank in 0..MAX_RECORDED_DIRECTORIES {
            inventory.register(&directory(rank));
        }
        assert_eq!(
            inventory.len(),
            MAX_RECORDED_DIRECTORIES,
            "case an inventory filled to the ceiling: expected {MAX_RECORDED_DIRECTORIES} \
             records, got {}",
            inventory.len()
        );

        inventory.register(&directory(MAX_RECORDED_DIRECTORIES));
        assert_eq!(
            inventory.len(),
            MAX_RECORDED_DIRECTORIES,
            "case one past the ceiling: expected the ceiling to hold, got {}",
            inventory.len()
        );
        assert!(
            !inventory.holds(&directory(0)),
            "case one past the ceiling: expected the oldest registration to be dropped"
        );
        assert!(
            inventory.holds(&directory(MAX_RECORDED_DIRECTORIES))
                && inventory.holds(&directory(1)),
            "case one past the ceiling: expected the newest and the next-oldest to be held"
        );
    }
}
