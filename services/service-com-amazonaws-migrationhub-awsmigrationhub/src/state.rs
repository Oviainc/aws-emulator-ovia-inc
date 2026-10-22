// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-migrationhub-awsmigrationhub service.
//!
//! Migration Hub's control plane is a two-level namespace: a *progress update
//! stream* (the migration tool's namespace, also the access-control resource)
//! holds *migration tasks* keyed by name. Both levels are `BTreeMap`s, so every
//! enumeration this service serves has one deterministic order — which is what
//! makes a pagination cursor that names the last key returned resumable.
//!
//! The stored task holds the MODELLED shapes (`Task`, `ResourceAttribute`,
//! `MigrationTaskUpdate`, `DiscoveredResource`) rather than private copies of
//! them, so a value written by one operation is the value the next reader
//! serialises, with no lossy conversion in between.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{DiscoveredResource, MigrationTaskUpdate, ResourceAttribute, Task};

/// A progress update stream: the namespace `ProgressUpdateStreamName` names, and
/// the migration tasks recorded inside it.
#[derive(Debug)]
pub struct ProgressUpdateStreamRecord {
    /// The name the stream was created with; also its key in the registry.
    pub name: String,
    /// When `CreateProgressUpdateStream` first created this stream. Not on the
    /// wire today; `ListProgressUpdateStreams` is the operation that will read it.
    pub created_at: SystemTime,
    /// Tasks in this stream, keyed by `MigrationTaskName`.
    pub tasks: BTreeMap<String, MigrationTaskRecord>,
}

impl ProgressUpdateStreamRecord {
    /// A brand-new, empty stream named `name`.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            created_at: SystemTime::now(),
            tasks: BTreeMap::new(),
        }
    }

    /// Register the task `name` in this stream, or leave the task already
    /// registered under that name exactly as it is (the import is idempotent on
    /// `(stream, name)`, which is the task's identity).
    ///
    /// `Err(AtCapacity)` when this stream already holds [`MAX_TASKS_PER_STREAM`]
    /// tasks and `name` is a new one; the caller turns that into its own modelled
    /// error rather than growing without bound.
    pub fn import_task(&mut self, name: &str) -> Result<&mut MigrationTaskRecord, AtCapacity> {
        if !self.tasks.contains_key(name) {
            if self.tasks.len() >= MAX_TASKS_PER_STREAM {
                return Err(AtCapacity);
            }
            let stream = self.name.clone();
            self.tasks
                .insert(name.to_string(), MigrationTaskRecord::new(stream, name));
        }
        Ok(self
            .tasks
            .get_mut(name)
            .expect("the task was just registered or was already present"))
    }
}

/// One migration task inside a progress update stream.
///
/// `ImportMigrationTask` registers it with no state at all: a task that has
/// never been notified has no `Task` and no `UpdateDateTime`, and
/// `DescribeMigrationTask` must omit both rather than invent a status.
#[derive(Debug)]
pub struct MigrationTaskRecord {
    /// The stream this task belongs to (the first half of its identity).
    pub progress_update_stream: String,
    /// The task's name within that stream (the second half of its identity).
    pub migration_task_name: String,
    /// The latest state `NotifyMigrationTaskState` wrote; `None` until the first
    /// notification arrives.
    pub task: Option<Task>,
    /// The caller-supplied `UpdateDateTime` of that latest notification, kept as
    /// the modelled timestamp so a reader returns the instant that was sent.
    pub update_date_time: Option<::aws_smithy_types::DateTime>,
    /// The latest notification's `NextUpdateSeconds` — the interval within which
    /// the tool promised its next update.
    pub next_update_seconds: Option<i32>,
    /// The attribute list `PutResourceAttributes` last wrote (it replaces, so
    /// this is a snapshot, not an accumulation).
    pub resource_attributes: Vec<ResourceAttribute>,
    /// Append-only update history, newest LAST. Backs `ListMigrationTaskUpdates`.
    pub updates: Vec<MigrationTaskUpdate>,
    /// Discovered resources associated with this task. Written by
    /// `AssociateDiscoveredResource` (still scaffolded); read by
    /// `ListMigrationTasks`' `ResourceName` filter.
    pub discovered_resources: Vec<DiscoveredResource>,
}

impl MigrationTaskRecord {
    /// Append `update` to this task's history, newest LAST, keeping the history
    /// bounded.
    ///
    /// Every notification a caller sends appends one entry, so the history is the
    /// one structure here that grows per REQUEST rather than per resource name:
    /// reaching [`MAX_UPDATES_PER_TASK`] evicts the OLDEST entries, so a tool that
    /// notifies forever keeps its recent history exact while the emulator's memory
    /// stays bounded.
    pub fn record_update(&mut self, update: MigrationTaskUpdate) {
        self.updates.push(update);
        if self.updates.len() > MAX_UPDATES_PER_TASK {
            self.updates.drain(..self.updates.len() - MAX_UPDATES_PER_TASK);
        }
    }

    /// An empty task, as `ImportMigrationTask` registers it.
    pub fn new(progress_update_stream: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            progress_update_stream: progress_update_stream.into(),
            migration_task_name: name.into(),
            task: None,
            update_date_time: None,
            next_update_seconds: None,
            resource_attributes: Vec::new(),
            updates: Vec::new(),
            discovered_resources: Vec::new(),
        }
    }
}

/// Per-service state for com-amazonaws-migrationhub-awsmigrationhub. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMigrationhubAwsmigrationhubState {
    /// Every progress update stream of this account, keyed (and so ordered) by
    /// `ProgressUpdateStreamName`.
    pub progress_update_streams: BTreeMap<String, ProgressUpdateStreamRecord>,
}

/// The largest number of progress update streams this emulator keeps for an
/// account. A stream is created by NAME, so a caller can mint one per request:
/// the cap is what stops that growing the process heap without limit. Chosen
/// (AWS publishes no per-account stream quota) well above any realistic migration
/// tool count, so it never shapes a normal run.
pub const MAX_PROGRESS_UPDATE_STREAMS: usize = 1_000;

/// The largest number of migration tasks this emulator keeps per stream, for the
/// same reason: `ImportMigrationTask` mints one per caller-chosen name. Chosen,
/// not an AWS quota.
pub const MAX_TASKS_PER_STREAM: usize = 10_000;

/// The largest number of update-history entries this emulator retains per task.
/// Reaching it evicts the OLDEST, so repeated notifications cannot grow memory
/// without limit. Chosen, not an AWS quota.
pub const MAX_UPDATES_PER_TASK: usize = 1_000;

/// The store cannot hold another resource of this kind; the caller answers with
/// its own modelled error rather than accepting unbounded growth.
#[derive(Debug, Clone, Copy)]
pub struct AtCapacity;

impl ComAmazonawsMigrationhubAwsmigrationhubState {
    /// Register the stream `name`, or leave the stream already registered under
    /// that name — and every task recorded in it — exactly as it is (creating a
    /// stream is idempotent on its name, which is its identity).
    ///
    /// `Err(AtCapacity)` when the registry already holds
    /// [`MAX_PROGRESS_UPDATE_STREAMS`] streams and `name` is a new one.
    pub fn create_stream(&mut self, name: &str) -> Result<(), AtCapacity> {
        if self.progress_update_streams.contains_key(name) {
            return Ok(());
        }
        if self.progress_update_streams.len() >= MAX_PROGRESS_UPDATE_STREAMS {
            return Err(AtCapacity);
        }
        self.progress_update_streams
            .insert(name.to_string(), ProgressUpdateStreamRecord::new(name));
        Ok(())
    }

    /// The stream named `name`, or `None` when this account has no such stream.
    pub fn stream(&self, name: &str) -> Option<&ProgressUpdateStreamRecord> {
        self.progress_update_streams.get(name)
    }

    /// The stream named `name`, mutably.
    pub fn stream_mut(&mut self, name: &str) -> Option<&mut ProgressUpdateStreamRecord> {
        self.progress_update_streams.get_mut(name)
    }

    /// The task `task` of stream `stream`, or `None` when either half is absent.
    pub fn task(&self, stream: &str, task: &str) -> Option<&MigrationTaskRecord> {
        self.stream(stream)?.tasks.get(task)
    }

    /// The task `task` of stream `stream`, mutably.
    pub fn task_mut(&mut self, stream: &str, task: &str) -> Option<&mut MigrationTaskRecord> {
        self.stream_mut(stream)?.tasks.get_mut(task)
    }

    /// Every task of every stream, in `(stream name, task name)` order.
    ///
    /// This is the one enumeration order the cursor codec below is defined
    /// against, so a page boundary means the same thing on every call.
    pub fn tasks_in_key_order(&self) -> impl Iterator<Item = &MigrationTaskRecord> {
        self.progress_update_streams
            .values()
            .flat_map(|stream| stream.tasks.values())
    }
}

/// The byte that separates the two halves of a cursor key.
///
/// `'|'` is excluded by BOTH modelled patterns — `ProgressUpdateStream` is
/// `^[^/:|\000-\037]+$` and `MigrationTaskName` is `^[^:|]+$` — so it cannot
/// occur inside either name and the split is unambiguous.
const CURSOR_SEPARATOR: char = '|';

/// Encode the `(stream, task)` key a page stopped on into an opaque token.
///
/// Base64 (standard alphabet) is what keeps the token inside the modelled
/// `Token` pattern `^[a-zA-Z0-9\/\+\=]{0,2048}$` whatever the names contain.
pub fn encode_cursor(stream: &str, task: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{stream}{CURSOR_SEPARATOR}{task}"))
}

/// Decode a token produced by [`encode_cursor`] back into its `(stream, task)`
/// key, or `None` when the token is not one this service issued.
pub fn decode_cursor(token: &str) -> Option<(String, String)> {
    let raw = ::aws_smithy_types::base64::decode(token).ok()?;
    let decoded = String::from_utf8(raw).ok()?;
    let (stream, task) = decoded.split_once(CURSOR_SEPARATOR)?;
    Some((stream.to_string(), task.to_string()))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMigrationhubAwsmigrationhubState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMigrationhubAwsmigrationhubState>)
}
