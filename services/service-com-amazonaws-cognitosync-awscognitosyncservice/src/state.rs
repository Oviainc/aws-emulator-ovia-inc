// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cognitosync-awscognitosyncservice service.
//!
//! Cognito Sync's primary resource is a *dataset*: a bag of key/value records that
//! belongs to one identity inside one identity pool ("A collection of data for an
//! identity pool. … A dataset is per identity", `smithy_query(query="doc",shape="Dataset")`).
//! Every value a response reports about a dataset — `NumRecords`, `DataStorage`,
//! `DatasetSyncCount`, `Count` — is DERIVED from the records stored here rather than
//! stored a second time, so a write followed by a read can never disagree.

use std::collections::{BTreeMap, HashMap, VecDeque};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{Dataset, Record};

/// A dataset's identity: `(IdentityPoolId, IdentityId, DatasetName)`.
///
/// Datasets are namespaced by pool AND identity, so the same dataset name under two
/// identities is two different datasets.
pub type DatasetKey = (String, String, String);

/// Build a [`DatasetKey`] from an operation's three `@httpLabel` members.
pub fn dataset_key(identity_pool_id: &str, identity_id: &str, dataset_name: &str) -> DatasetKey {
    (
        identity_pool_id.to_string(),
        identity_id.to_string(),
        dataset_name.to_string(),
    )
}

/// The server's clock, as the `Date` timestamps in this model want it.
pub fn now() -> DateTime {
    DateTime::from(::std::time::SystemTime::now())
}

/// One stored record of a dataset. The record's `Key` is the map key it is stored
/// under, so it is never held twice.
#[derive(Debug, Clone)]
pub struct RecordEntry {
    pub value: Option<String>,
    pub sync_count: i64,
    pub last_modified_date: DateTime,
    pub last_modified_by: Option<String>,
    pub device_last_modified_date: Option<DateTime>,
}

impl RecordEntry {
    /// The bytes this record contributes to `Dataset.DataStorage` ("Total size in bytes
    /// of the records in this dataset"): its key plus its value.
    pub fn stored_bytes(&self, key: &str) -> i64 {
        let value_len = self.value.as_ref().map_or(0, String::len);
        i64::try_from(key.len() + value_len).unwrap_or(i64::MAX)
    }

    /// This record as the modelled `Record` shape.
    pub fn to_shape(&self, key: &str) -> Record {
        Record {
            key: Some(key.to_string()),
            value: self.value.clone(),
            sync_count: Some(self.sync_count),
            last_modified_date: Some(self.last_modified_date),
            last_modified_by: self.last_modified_by.clone(),
            device_last_modified_date: self.device_last_modified_date,
        }
    }
}

/// One stored dataset.
///
/// `records` is a `BTreeMap` so every listing — and therefore every page boundary —
/// is in the same order on every call.
#[derive(Debug, Clone)]
pub struct DatasetEntry {
    pub creation_date: DateTime,
    pub last_modified_date: DateTime,
    pub last_modified_by: Option<String>,
    pub sync_count: i64,
    pub records: BTreeMap<String, RecordEntry>,
}

impl DatasetEntry {
    /// A dataset created, empty, at `created_at`. `UpdateRecords` creates one
    /// implicitly: "Datasets are automatically created if they don't exist."
    pub fn created_at(created_at: DateTime) -> Self {
        Self {
            creation_date: created_at,
            last_modified_date: created_at,
            last_modified_by: None,
            sync_count: 0,
            records: BTreeMap::new(),
        }
    }

    /// `Dataset.NumRecords` — "Number of records in this dataset."
    pub fn num_records(&self) -> i64 {
        i64::try_from(self.records.len()).unwrap_or(i64::MAX)
    }

    /// `Dataset.DataStorage` — "Total size in bytes of the records in this dataset."
    pub fn data_storage(&self) -> i64 {
        self.records
            .iter()
            .map(|(key, record)| record.stored_bytes(key))
            .sum()
    }

    /// This dataset as the modelled `Dataset` shape, under the key it is stored at.
    pub fn to_shape(&self, key: &DatasetKey) -> Dataset {
        let (_identity_pool_id, identity_id, dataset_name) = key;
        Dataset {
            identity_id: Some(identity_id.clone()),
            dataset_name: Some(dataset_name.clone()),
            creation_date: Some(self.creation_date),
            last_modified_date: Some(self.last_modified_date),
            last_modified_by: self.last_modified_by.clone(),
            data_storage: Some(self.data_storage()),
            num_records: Some(self.num_records()),
        }
    }
}

/// How many sync-session tokens this emulator remembers.
///
/// Not an AWS rule: a bound on the emulator's own memory, since `ListRecords` mints a
/// token on every call and the real service's tokens carry their own expiration. The
/// oldest token is forgotten first, and a forgotten token is refused exactly like one
/// that was never minted.
pub const MAX_TRACKED_SYNC_SESSIONS: usize = 10_000;

/// How many deletions this emulator remembers, so `ListRecords` can report a dataset
/// `DeleteDataset` removed.
///
/// Not an AWS rule: a bound on the emulator's own memory, since a caller can repeat
/// create/delete for any dataset name it invents. The oldest tombstone is forgotten
/// first, and a forgotten one reads exactly like a dataset that never existed.
pub const MAX_TRACKED_DELETED_DATASETS: usize = 1024;

/// How many datasets this emulator holds for ONE identity.
///
/// Not an AWS rule: a bound on the emulator's own memory. Each dataset holds up to
/// [`MAX_DATASET_RECORDS`] records and up to the 1 MB the `Dataset` doc states, and the
/// (pool, identity, name) key is chosen by the caller, so the count needs a ceiling of
/// its own. `UpdateRecords` refuses to create one past it.
pub const MAX_DATASETS_PER_IDENTITY: usize = 64;

/// How many datasets this emulator holds in total, across every identity — the same
/// memory bound as [`MAX_DATASETS_PER_IDENTITY`], one level up. Not an AWS rule.
pub const MAX_DATASETS: usize = 1024;

/// How many records one dataset holds.
///
/// Not an AWS rule: the 1 MB the `Dataset` doc states bounds the BYTES of a dataset, so
/// without this a dataset could hold a million one-byte-key records and every
/// `UpdateRecords` would re-scan them under the store lock. `UpdateRecords` refuses a
/// batch that would push a dataset past it.
pub const MAX_DATASET_RECORDS: usize = 4096;

/// The largest page any list operation of this service answers with.
///
/// Not an AWS rule: the server, not the caller, decides how much one response may carry.
/// A `MaxResults` above it is served at this size, and a request that omits `MaxResults`
/// is served at this size too — in both cases `NextToken` carries the rest.
pub const MAX_PAGE_SIZE: usize = 1024;

/// What every handler of this service says when its store is not mounted on the engine
/// it was built from — single-homed here so the five handlers cannot word it differently.
pub const STORE_UNAVAILABLE: &str = "the Cognito Sync dataset store is not mounted on this engine";

/// Per-service state for com-amazonaws-cognitosync-awscognitosyncservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCognitosyncAwscognitosyncserviceState {
    /// Every stored dataset, ordered so listings and pages are deterministic.
    pub datasets: BTreeMap<DatasetKey, DatasetEntry>,
    /// For a dataset `DeleteDataset` removed: the `DatasetSyncCount` it carried when it
    /// was deleted. `ListRecords` answers `DatasetDeletedAfterRequestedSyncCount` from
    /// this. Reached through [`Self::deletion_sync_count`] /
    /// [`Self::record_deletion`] / [`Self::forget_deletion`], which keep it bounded.
    deleted_datasets: HashMap<DatasetKey, i64>,
    /// The order deletions were recorded in, so the oldest tombstone is evicted first
    /// once [`MAX_TRACKED_DELETED_DATASETS`] is reached.
    deletion_order: VecDeque<DatasetKey>,
    /// Sync sessions `ListRecords` minted, by token: the dataset each one is good for.
    /// `UpdateRecords` requires "The SyncSessionToken returned by a previous call to
    /// ListRecords for this dataset and identity".
    sync_sessions: HashMap<String, DatasetKey>,
    /// The order tokens were minted in, so the oldest is evicted first once
    /// [`MAX_TRACKED_SYNC_SESSIONS`] is reached.
    session_order: VecDeque<String>,
    /// Counter behind a minted token's session id; makes every token unique.
    next_session_id: u64,
}

impl ComAmazonawsCognitosyncAwscognitosyncserviceState {
    /// Mint a sync-session token for `key` and remember it.
    ///
    /// The token is opaque to the client, as the real service's is ("A token containing
    /// a session ID, identity ID, and expiration"): the pairing with the dataset lives
    /// here, so a token cannot be forged by composing one.
    pub fn mint_sync_session(&mut self, key: &DatasetKey, minted_at: DateTime) -> String {
        self.next_session_id = self.next_session_id.wrapping_add(1);
        let token = format!(
            "{:016x}{:016x}",
            u64::try_from(minted_at.secs()).unwrap_or(0),
            self.next_session_id
        );
        self.sync_sessions.insert(token.clone(), key.clone());
        self.session_order.push_back(token.clone());
        while self.session_order.len() > MAX_TRACKED_SYNC_SESSIONS {
            if let Some(oldest) = self.session_order.pop_front() {
                self.sync_sessions.remove(&oldest);
            }
        }
        token
    }

    /// The dataset a minted sync-session token is good for, or `None` when this
    /// emulator never minted (or no longer remembers) it.
    pub fn sync_session_dataset(&self, token: &str) -> Option<&DatasetKey> {
        self.sync_sessions.get(token)
    }

    /// Remember that `key` was deleted at `sync_count`, forgetting the oldest tombstone
    /// once [`MAX_TRACKED_DELETED_DATASETS`] are held.
    pub fn record_deletion(&mut self, key: &DatasetKey, sync_count: i64) {
        if self.deleted_datasets.insert(key.clone(), sync_count).is_none() {
            self.deletion_order.push_back(key.clone());
        }
        while self.deletion_order.len() > MAX_TRACKED_DELETED_DATASETS {
            if let Some(oldest) = self.deletion_order.pop_front() {
                self.deleted_datasets.remove(&oldest);
            }
        }
    }

    /// Drop `key`'s tombstone: the dataset exists again, so it was not deleted.
    pub fn forget_deletion(&mut self, key: &DatasetKey) {
        if self.deleted_datasets.remove(key).is_some() {
            self.deletion_order.retain(|tracked| tracked != key);
        }
    }

    /// The `DatasetSyncCount` `key` carried when it was deleted, when that deletion is
    /// still remembered.
    pub fn deletion_sync_count(&self, key: &DatasetKey) -> Option<i64> {
        self.deleted_datasets.get(key).copied()
    }

    /// How many datasets are stored for one identity.
    pub fn dataset_count_of_identity(&self, identity_pool_id: &str, identity_id: &str) -> usize {
        self.datasets_of_identity(identity_pool_id, identity_id).count()
    }

    /// Every dataset stored for one identity, in key order.
    pub fn datasets_of_identity<'a>(
        &'a self,
        identity_pool_id: &'a str,
        identity_id: &'a str,
    ) -> impl Iterator<Item = (&'a DatasetKey, &'a DatasetEntry)> + 'a {
        self.datasets.iter().filter(move |((pool, identity, _), _)| {
            pool == identity_pool_id && identity == identity_id
        })
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCognitosyncAwscognitosyncserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCognitosyncAwscognitosyncserviceState>)
}
