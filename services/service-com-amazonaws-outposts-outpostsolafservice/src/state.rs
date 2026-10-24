// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-outposts-outpostsolafservice service, plus
//! the rules more than one operation in the site + Outpost lifecycle applies: id
//! and ARN minting, resolving the `id OR ARN` identifier forms the modelled
//! `@httpLabel` patterns accept, projecting a resource's tag map and an Outpost's
//! two Availability Zone identities, materialising the Outpost lifecycle transition
//! two Availability Zone identities, materialising the Outpost lifecycle transition,
//! the quote expiry and the capacity-task progression the clock owes, cutting an
//! ordered list into pages with the modelled `Token` the next page resumes from, and
//! the two rules this service words its refusals by — the `NotFoundException`
//! identifier echo and the value-omitting `ValidationException` report.

use std::collections::{BTreeMap, HashMap, VecDeque};

use sha2::{Digest, Sha256};
use smithy_server_runtime::{Violation, ViolationKind};

use crate::ops::types::{
    Address, CapacityTaskFailure, InstanceTypeCapacity, InstancesToExclude, Outpost,
    PrivateConnectivityConfig, Quote, Site,
};

/// The region every minted ARN falls back to when the request carries no SigV4
/// credential scope to read one from (an unsigned, anonymous call).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace of this service, as the model's `awsService` trait declares
/// it (`arnNamespace: outposts`) — the third segment of every ARN it mints.
pub const ARN_NAMESPACE: &str = "outposts";

/// The `LifeCycleStatus` a freshly created Outpost carries.
///
/// AWS answers `CreateOutpost` with an Outpost that is still being provisioned, so
/// the status a create reports — and every read of it reports until the transition
/// below falls due — is the non-terminal one. The model constrains
/// `LifeCycleStatus` without declaring a value set (len 1..20, pattern
/// `^[ A-Za-z]+$`), and both values this service uses satisfy it.
pub const LIFE_CYCLE_STATUS_PENDING: &str = "PENDING";

/// The `LifeCycleStatus` an Outpost settles in: the terminal, ready status it
/// holds once provisioning is done, which no later call moves it out of.
pub const LIFE_CYCLE_STATUS_ACTIVE: &str = "ACTIVE";

/// How long after `CreateOutpost` the Outpost's
/// [`LIFE_CYCLE_STATUS_PENDING`] -> [`LIFE_CYCLE_STATUS_ACTIVE`] transition falls
/// due.
///
/// AWS publishes no duration for provisioning an Outpost, so this span is the
/// emulator's own: short enough that a workflow polling for a ready Outpost
/// finishes at once, and long enough that a caller acting straight after the create
/// still observes the pending status the create reported. It is never zero —
/// a transition the clock owes is owed at an instant, not on the next read.
pub const OUTPOST_PROVISIONING: std::time::Duration = std::time::Duration::from_millis(500);

/// The `PrivateConnectivityStatus` an Outpost's private connectivity carries once
/// it has been configured.
///
/// The modelled `PrivateConnectivityStatus` enum declares exactly two values,
/// `ENABLED` and `DISABLED` (see
/// [`crate::ops::types::shared_types::PRIVATE_CONNECTIVITY_STATUS_VALUES`]), and neither is a
/// provisioning value — so a configuration that exists is already in a terminal
/// status and the clock owes no transition on it.
pub const PRIVATE_CONNECTIVITY_ENABLED: &str = "ENABLED";

/// The `QuoteStatus` a freshly created quote carries.
///
/// `CreateQuote` answers with a quote that exists to be read and ordered from, so
/// the status a create reports is the one a quote is usable in. The modelled
/// `QuoteStatus` enum declares exactly three values — `CREATED`,
/// `ORDER_SUBMITTED` and `EXPIRED` — and a quote reaches the other two only by
/// something happening to it: an order being submitted from it, or its own
/// `ExpirationDate` passing.
pub const QUOTE_STATUS_CREATED: &str = "CREATED";

/// The `QuoteStatus` a quote holds once the `ExpirationDate` it reports has
/// passed. See [`QUOTE_STATUS_CREATED`] and
/// [`ComAmazonawsOutpostsOutpostsolafserviceState::settle_quotes`].
pub const QUOTE_STATUS_EXPIRED: &str = "EXPIRED";

/// How long after `CreateQuote` the quote's `ExpirationDate` falls.
///
/// AWS documents the member only as "The date the quote expires" and publishes no
/// validity period, so this span is the emulator's own. It is a resource LIFETIME,
/// not a provisioning span: a quote is created in order to be read and ordered
/// from, so the window has to be long enough that every call a workflow makes
/// against a quote it just created still sees it live, while the quote still
/// carries a real expiry a caller can read and act on.
pub const QUOTE_VALIDITY: std::time::Duration =
    std::time::Duration::from_secs(30 * 24 * 60 * 60);

/// A persisted site: the modelled [`Site`] this service answers with, plus the two
/// whole [`Address`] values `Site` itself does not carry (it exposes only the
/// operating address's country, state and city) and the creation order every list
/// pages in.
#[derive(Debug, Clone)]
pub struct SiteRecord {
    /// Position in this store's creation order — the sort key every list and every
    /// page token is expressed in, so a token stays correct across calls.
    pub sequence: u64,
    /// The site as `CreateSite` / `GetSite` / `ListSites` return it.
    pub site: Site,
    /// The full operating address the caller supplied, which `Site` projects down
    /// to `OperatingAddress{CountryCode,StateOrRegion,City}`.
    pub operating_address: Option<Address>,
    /// The full shipping address the caller supplied. `Site` carries no shipping
    /// member at all, so this is the only place it is kept.
    pub shipping_address: Option<Address>,
}

/// A persisted Outpost: the modelled [`Outpost`] plus its creation order, the
/// instant its lifecycle transition falls due, and the private connectivity
/// configuration created for it.
#[derive(Debug, Clone)]
pub struct OutpostRecord {
    /// See [`SiteRecord::sequence`].
    pub sequence: u64,
    /// The Outpost as `CreateOutpost` / `GetOutpost` / `ListOutposts` return it.
    pub outpost: Outpost,
    /// The private connectivity configuration of this Outpost, or `None` for an
    /// Outpost nobody has created one for.
    ///
    /// Private connectivity is an attribute of ONE Outpost — the modelled routes
    /// are `/outposts/{OutpostId}/privateConnectivity` — so it is kept on the
    /// Outpost's own record: the create writes it here, the read answers it from
    /// here, and deleting the Outpost takes it with the Outpost.
    pub private_connectivity: Option<PrivateConnectivityConfig>,
    /// When this Outpost's `PENDING` -> `ACTIVE` transition falls due, or `None`
    /// once it has been materialised and the status is terminal. Stored as an
    /// instant so nothing ever sleeps for it: the transition is applied by the
    /// first call made at or after it, which reports exactly the status the
    /// Outpost would hold had nobody called at all.
    pub active_at: Option<std::time::Instant>,
}

/// The `CapacityTaskStatus` a capacity task carries the moment `StartCapacityTask`
/// accepts it: "the capacity task was created and is awaiting the next step by
/// Amazon Web Services Outposts", as the model documents the value.
pub const CAPACITY_TASK_REQUESTED: &str = "REQUESTED";

/// The `CapacityTaskStatus` a capacity task carries while it runs — the model's
/// "the capacity task is running and cannot be cancelled".
pub const CAPACITY_TASK_IN_PROGRESS: &str = "IN_PROGRESS";

/// The terminal `CapacityTaskStatus` of a capacity task that ran: the model's "the
/// capacity task has completed successfully". No later call moves a task out of it.
pub const CAPACITY_TASK_COMPLETED: &str = "COMPLETED";

/// The terminal `CapacityTaskStatus` of a task that could not be completed.
pub const CAPACITY_TASK_FAILED: &str = "FAILED";

/// The terminal `CapacityTaskStatus` of a cancelled task.
pub const CAPACITY_TASK_CANCELLED: &str = "CANCELLED";

/// Every TERMINAL `CapacityTaskStatus` the model declares: a task in one of these
/// is done, and the Outpost and order it belongs to can take another task. The
/// other three modelled values (`REQUESTED`, `IN_PROGRESS`,
/// `WAITING_FOR_EVACUATION`) are statuses Amazon Web Services Outposts is still
/// working through, so a task in one of them is ACTIVE.
pub const CAPACITY_TASK_TERMINAL: &[&str] = &[
    CAPACITY_TASK_COMPLETED,
    CAPACITY_TASK_FAILED,
    CAPACITY_TASK_CANCELLED,
];

// The `CapacityTaskStatus` values a task this emulator started never reaches, and
// why, stated once so no handler invents one:
//
// * `WAITING_FOR_EVACUATION` and the `CapacityTaskFailure` type
//   `BLOCKING_INSTANCES_NOT_EVACUATED` describe a task held up by running EC2
//   instances. This emulator runs no instances on an Outpost, so no task is ever
//   blocked by one and none waits.
// * `FAILED` needs a failure something produced; nothing in this slice can fail a
//   task, so `Failed` stays absent rather than carrying an invented reason.
// * `CANCELLATION_IN_PROGRESS` and `CANCELLED` belong to `CancelCapacityTask`,
//   which is still a scaffold stub.

/// How long after the call that started it each step of a capacity task's
/// `REQUESTED` -> `IN_PROGRESS` -> `COMPLETED` progression falls due.
///
/// AWS publishes no duration for running a capacity task, so — exactly as
/// [`OUTPOST_PROVISIONING`] — this span is the emulator's own: short enough that a
/// workflow polling for a finished task finishes at once, and never zero, so a
/// caller reading straight after `StartCapacityTask` still observes the
/// `REQUESTED` status the start reported.
pub const CAPACITY_TASK_STEP: std::time::Duration = std::time::Duration::from_millis(500);

/// One status transition a capacity task owes the clock.
#[derive(Debug, Clone)]
pub struct CapacityTaskTransition {
    /// When the transition falls due, as a monotonic instant: nothing ever sleeps
    /// for it, the first call made at or after it materialises it.
    pub due: std::time::Instant,
    /// The wall clock the transition is owed AT, which is what the task's
    /// `LastModifiedDate` (and, for the terminal step, `CompletionDate`) reports —
    /// so a late poll reports when the task really changed, not when it was polled.
    pub at: std::time::SystemTime,
    /// The `CapacityTaskStatus` the task holds from that instant on.
    pub status: String,
}

/// A persisted capacity task: every member `StartCapacityTask` / `GetCapacityTask`
/// report, plus its creation order and the transitions the clock still owes it.
#[derive(Debug, Clone)]
pub struct CapacityTaskRecord {
    /// See [`SiteRecord::sequence`].
    pub sequence: u64,
    /// The minted `CapacityTaskId` (`cap-…`) this record is keyed by.
    pub capacity_task_id: String,
    /// The bare `OutpostId` of the Outpost the task runs on — the parent the
    /// task's route nests it under.
    pub outpost_id: String,
    /// The `OrderId` the caller associated the task with, if any.
    pub order_id: Option<String>,
    /// The `AssetId` the caller named, if any.
    pub asset_id: Option<String>,
    /// The instance pools the caller requested, reported back as
    /// `RequestedInstancePools`.
    pub requested_instance_pools: Vec<InstanceTypeCapacity>,
    /// The instances the caller said must not be stopped.
    pub instances_to_exclude: Option<InstancesToExclude>,
    /// Whether the task was requested as a dry run.
    pub dry_run: bool,
    /// What the caller asked for when an instance blocks the task.
    pub task_action_on_blocking_instances: Option<String>,
    /// The `CapacityTaskStatus` the task holds now, as of the last settle.
    pub status: String,
    /// Why the task failed. See [`CAPACITY_TASK_FAILED`].
    pub failed: Option<CapacityTaskFailure>,
    /// When `StartCapacityTask` created the task (`CreationDate`).
    pub created_at: std::time::SystemTime,
    /// When the task last changed (`LastModifiedDate`).
    pub last_modified_at: std::time::SystemTime,
    /// When the task completed (`CompletionDate`), absent until it does.
    pub completed_at: Option<std::time::SystemTime>,
    /// The transitions still owed, in the order they fall due. Empty once the task
    /// has settled in a terminal status.
    pub pending: VecDeque<CapacityTaskTransition>,
}

impl CapacityTaskRecord {
    /// Whether this task is ACTIVE: it is in a status Amazon Web Services Outposts
    /// is still working through, so the Outpost (and the order) it belongs to
    /// cannot take another one. A task in a terminal status is not active.
    pub fn is_active(&self) -> bool {
        !CAPACITY_TASK_TERMINAL.contains(&self.status.as_str())
    }
}

/// The transitions a capacity task started at `now` owes the clock: one
/// [`CAPACITY_TASK_STEP`] to `IN_PROGRESS`, and another to `COMPLETED`.
///
/// Both instants are fixed when the task starts, so the status a read reports is
/// the one the task holds at the instant it is asked for — the same answer whether
/// or not anybody polled in between.
pub fn capacity_task_progression(now: std::time::SystemTime) -> VecDeque<CapacityTaskTransition> {
    let started = std::time::Instant::now();
    [CAPACITY_TASK_IN_PROGRESS, CAPACITY_TASK_COMPLETED]
        .into_iter()
        .enumerate()
        .map(|(step, status)| {
            let elapsed = CAPACITY_TASK_STEP * (u32::try_from(step).unwrap_or(0) + 1);
            CapacityTaskTransition {
                due: started + elapsed,
                at: now + elapsed,
                status: status.to_string(),
            }
        })
        .collect()
}

/// Project a stored [`CapacityTaskRecord`] onto an operation's output shape.
///
/// `StartCapacityTask` and `GetCapacityTask` declare the SAME thirteen members for
/// a capacity task, as two distinct generated structures. Stating the projection
/// once, as a macro over the output type, is what keeps the two from drifting
/// apart on what a capacity task reports, the way [`page`] keeps the two lists from
/// drifting apart on how they page.
///
/// `RequestedInstancePools` is always reported, empty list and all, because the
/// pools are part of the task the caller asked for; `OrderId`, `AssetId`,
/// `InstancesToExclude` and `TaskActionOnBlockingInstances` are reported only when
/// the caller named them, and `Failed` only when the task failed.
#[macro_export]
macro_rules! capacity_task_projection {
    ($output:ident, $record:expr) => {{
        let record: &$crate::state::CapacityTaskRecord = $record;
        $output {
            capacity_task_id: Some(record.capacity_task_id.clone()),
            outpost_id: Some(record.outpost_id.clone()),
            order_id: record.order_id.clone(),
            asset_id: record.asset_id.clone(),
            requested_instance_pools: Some(record.requested_instance_pools.clone()),
            instances_to_exclude: record.instances_to_exclude.clone(),
            dry_run: record.dry_run,
            capacity_task_status: Some(record.status.clone()),
            failed: record.failed.clone(),
            creation_date: Some(::aws_smithy_types::DateTime::from(record.created_at)),
            completion_date: record
                .completed_at
                .map(::aws_smithy_types::DateTime::from),
            last_modified_date: Some(::aws_smithy_types::DateTime::from(record.last_modified_at)),
            task_action_on_blocking_instances: record.task_action_on_blocking_instances.clone(),
        }
    }};
}

/// A persisted quote: the modelled [`Quote`] plus its creation order.
///
/// A quote carries its own `CreatedDate` and `ExpirationDate`, so the instant its
/// `CREATED` -> `EXPIRED` transition falls due is the `ExpirationDate` the stored
/// quote already reports — there is no second copy of it to drift from.
#[derive(Debug, Clone)]
pub struct QuoteRecord {
    /// See [`SiteRecord::sequence`].
    pub sequence: u64,
    /// The quote as `CreateQuote` / `GetQuote` return it.
    pub quote: Quote,
}

/// Per-service state for com-amazonaws-outposts-outpostsolafservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsOutpostsOutpostsolafserviceState {
    /// Every persisted site, keyed by its bare `SiteId` (`os-…`), which is how
    /// `GetSite`, `CreateOutpost` and `DeleteSite` resolve one.
    pub sites: BTreeMap<String, SiteRecord>,
    /// Every persisted Outpost, keyed by its bare `OutpostId` (`op-…`).
    pub outposts: BTreeMap<String, OutpostRecord>,
    /// Every persisted capacity task, keyed by its `CapacityTaskId` (`cap-…`),
    /// which is how `GetCapacityTask` resolves one.
    pub capacity_tasks: BTreeMap<String, CapacityTaskRecord>,
    /// Every persisted quote, keyed by its bare `QuoteId` (`oq-…`), which is how
    /// `GetQuote` resolves one.
    pub quotes: BTreeMap<String, QuoteRecord>,
    /// How many resources this store has created. Doubles as the monotonically
    /// increasing creation order every record records and every page token names,
    /// and as part of each minted id's seed so no two ids collide.
    pub created: u64,
}

impl ComAmazonawsOutpostsOutpostsolafserviceState {
    /// Materialise every Outpost lifecycle transition the clock has already made
    /// due.
    ///
    /// Every operation that reports or filters on an Outpost calls this FIRST, so a
    /// read answers the status the Outpost holds at the instant it is asked for and
    /// never a stale one. It writes only what the clock already owed at that same
    /// instant, so reading changes nothing a later caller would not have seen
    /// anyway, and a settled Outpost is left alone.
    pub fn settle_outposts(&mut self) {
        let now = std::time::Instant::now();
        for record in self.outposts.values_mut() {
            if record.active_at.is_some_and(|due| now >= due) {
                record.outpost.life_cycle_status = Some(LIFE_CYCLE_STATUS_ACTIVE.to_string());
                record.active_at = None;
            }
        }
    }

    /// Materialise every capacity-task transition the clock has already made due.
    ///
    /// Every operation that reports a capacity task calls this FIRST, for the same
    /// reason [`Self::settle_outposts`] exists: a read answers the status the task
    /// holds at the instant it is asked for. It applies only what the clock already
    /// owed at that instant — every step whose due instant has passed, so a caller
    /// that polls late sees the same terminal status, with the same
    /// `LastModifiedDate` and `CompletionDate`, as one that polled all along — and a
    /// task that has settled in a terminal status is left alone.
    pub fn settle_capacity_tasks(&mut self) {
        let now = std::time::Instant::now();
        for record in self.capacity_tasks.values_mut() {
            while record.pending.front().is_some_and(|next| now >= next.due) {
                let Some(next) = record.pending.pop_front() else {
                    break;
                };
                record.last_modified_at = next.at;
                if next.status == CAPACITY_TASK_COMPLETED {
                    record.completed_at = Some(next.at);
                }
                record.status = next.status;
            }
        }
    }

    /// Materialise every quote expiry the clock has already made due.
    ///
    /// Every operation that reports a quote calls this FIRST, for the same reason
    /// [`Self::settle_outposts`] exists: a read answers the status the quote holds
    /// at the instant it is asked for. A quote expires when the
    /// `ExpirationDate` it itself reports has passed, so this writes only what the
    /// clock already owed at that same instant and nothing a later caller would not
    /// have seen anyway.
    ///
    /// Only a quote still in [`QUOTE_STATUS_CREATED`] is moved: the expiry of a
    /// quote an order was already submitted from would be rewriting history, and
    /// one already `EXPIRED` has nowhere left to go.
    pub fn settle_quotes(&mut self) {
        let now = now_timestamp().as_secs_f64();
        for record in self.quotes.values_mut() {
            let due = record
                .quote
                .expiration_date
                .as_ref()
                .is_some_and(|expiration| expiration.as_secs_f64() <= now);
            if due && record.quote.quote_status.as_deref() == Some(QUOTE_STATUS_CREATED) {
                record.quote.quote_status = Some(QUOTE_STATUS_EXPIRED.to_string());
            }
        }
    }

    /// The capacity task that is still ACTIVE on `outpost_id`, if any.
    ///
    /// "You can have one active capacity task for each order and each Outpost", so
    /// this is what `StartCapacityTask` refuses a second one against. Callers settle
    /// first: a task the clock has already completed holds nothing up.
    pub fn active_capacity_task_on_outpost(&self, outpost_id: &str) -> Option<&CapacityTaskRecord> {
        self.capacity_tasks
            .values()
            .find(|task| task.outpost_id == outpost_id && task.is_active())
    }

    /// The capacity task that is still ACTIVE on `order_id`, if any. See
    /// [`Self::active_capacity_task_on_outpost`] — the same rule, stated for the
    /// other resource a task is one-at-a-time per.
    pub fn active_capacity_task_on_order(&self, order_id: &str) -> Option<&CapacityTaskRecord> {
        self.capacity_tasks
            .values()
            .find(|task| task.order_id.as_deref() == Some(order_id) && task.is_active())
    }

    /// Mint a fresh resource id of the form `<prefix>-<17 lowercase hex chars>`,
    /// and return it with the creation sequence to record on the new resource.
    ///
    /// `OutpostId` (`^op-[a-f0-9]{17}$`), `SiteId` (`^os-[a-f0-9]{17}$`),
    /// `QuoteId` (`^oq-[a-f0-9]{17}$`) and `CapacityTaskId` (`^cap-[a-f0-9]{17}$`)
    /// are each a short literal prefix and exactly 17 lowercase hex characters, so
    /// one helper serves them all. `created` advances on every call and is hashed
    /// into the seed with the wall clock and the process id, so repeated calls never
    /// return the same id.
    pub fn mint_id(&mut self, prefix: &str) -> (String, u64) {
        self.created = self.created.wrapping_add(1);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let seed = format!("{prefix}|{}|{}|{nanos}", self.created, std::process::id());
        let digest = Sha256::digest(seed.as_bytes());
        let mut hex = hex::encode(&digest[..9]);
        // 9 bytes encode to 18 characters; the patterns want exactly 17.
        hex.truncate(17);
        (format!("{prefix}-{hex}"), self.created)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsOutpostsOutpostsolafserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsOutpostsOutpostsolafserviceState>)
}

/// The region to mint this request's ARNs in: the region of its SigV4 credential
/// scope, or [`DEFAULT_REGION`] when the request is unsigned.
///
/// A scope region is honoured only when it can appear in an Outposts ARN at all —
/// the modelled `SiteArn` / `OutpostArn` patterns pin that segment to `[a-z\d-]+`
/// — so a hostile or nonsense scope can never make this service mint an ARN its
/// own model rejects.
pub fn request_region(parts: &http::request::Parts) -> String {
    emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A `SiteArn`: `arn:aws:outposts:<region>:<account>:site/os-…`.
pub fn site_arn(region: &str, site_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("site/{site_id}"))
}

/// An `OutpostArn`: `arn:aws:outposts:<region>:<account>:outpost/op-…`.
pub fn outpost_arn(region: &str, outpost_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("outpost/{outpost_id}"))
}

/// The bare resource id an Outposts identifier names, whether it arrived as the id
/// or as the full ARN.
///
/// Every identifier member in this API is modelled
/// `^(arn:…:<kind>/)?(<prefix>-[a-f0-9]{17})$` — the ARN prefix is optional — so one
/// resolver serves `SiteId`, `OutpostId` and the ARN forms alike: take the segment
/// after the last `/` when the value is an ARN, then require the bare id to be the
/// `<prefix>-` plus exactly 17 lowercase hex characters the pattern allows.
/// `None` means the value is not an identifier this service could have minted.
pub fn resolve_id(identifier: &str, prefix: &str) -> Option<String> {
    let bare = match identifier.starts_with("arn:") {
        true => identifier.rsplit('/').next()?,
        false => identifier,
    };
    let suffix = bare.strip_prefix(prefix)?.strip_prefix('-')?;
    let well_formed = suffix.len() == 17
        && suffix
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    well_formed.then(|| bare.to_string())
}

/// The `os-…` id a `SiteId` names. See [`resolve_id`].
pub fn resolve_site_id(identifier: &str) -> Option<String> {
    resolve_id(identifier, "os")
}

/// The `op-…` id an `OutpostId` names. See [`resolve_id`].
pub fn resolve_outpost_id(identifier: &str) -> Option<String> {
    resolve_id(identifier, "op")
}

/// The `oq-…` id a `QuoteIdentifier` names. See [`resolve_id`].
pub fn resolve_quote_id(identifier: &str) -> Option<String> {
    resolve_id(identifier, "oq")
}

/// Now, as the modelled `ISO8601Timestamp` members of this API carry an instant.
///
/// One helper so every timestamp this service reports is read off the same clock
/// and in the same units, whether it is being stored or compared against a stored
/// one.
pub fn now_timestamp() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(std::time::SystemTime::now())
}

/// `at` as a modelled `ISO8601Timestamp`. See [`now_timestamp`].
pub fn timestamp(at: std::time::SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The `(resource type, resource id)` an Outposts resource ARN names, e.g.
/// `("site", "os-…")` for `arn:aws:outposts:<region>:<account>:site/os-…`.
///
/// Every `Arn` member in this API is modelled
/// `^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:([a-z\d-]+)/)[a-z]{2,8}-[a-f0-9]{17}$`,
/// so the resource field is always `<type>/<id>` and the service field is always
/// this service's own ARN namespace. `None` means the value is not an ARN of this
/// service at all.
pub fn resource_arn_parts(arn: &str) -> Option<(String, String)> {
    let mut fields = arn.strip_prefix("arn:")?.splitn(5, ':');
    let _partition = fields.next()?;
    let service = fields.next()?;
    let _region = fields.next()?;
    let _account = fields.next()?;
    let (resource_type, resource_id) = fields.next()?.split_once('/')?;
    (service == ARN_NAMESPACE).then(|| (resource_type.to_string(), resource_id.to_string()))
}

/// Which Availability Zone identifiers a request addressed to a region may name.
///
/// AWS names an Availability Zone in two ways, and both pin it to ONE region: the
/// zone NAME is the region code plus a zone suffix (`us-east-1a`), while the zone
/// ID is the region's abbreviation plus `-az<n>` (`us-east-1` -> `use1-az1`), which
/// [`region_abbreviation`] derives from the region code itself. So a request
/// addressed to one region cannot name a zone of another.
///
/// Whether `zone` is an Availability Zone NAME of `region`: the region code plus a
/// suffix that is not itself more digits of a longer region code (so `us-east-1`
/// does not claim `us-east-12a`).
pub fn zone_name_in_region(zone: &str, region: &str) -> bool {
    zone.to_ascii_lowercase()
        .strip_prefix(region)
        .is_some_and(|suffix| {
            !suffix.is_empty() && !suffix.starts_with(|character: char| character.is_ascii_digit())
        })
}

/// Whether `zone_id` is an Availability Zone ID of `region`: the region's
/// abbreviation followed by the zone's own part. A region
/// [`region_abbreviation`] cannot abbreviate is a region no zone id can be judged
/// against, so every id passes rather than being refused on a guess.
pub fn zone_id_in_region(zone_id: &str, region: &str) -> bool {
    match region_abbreviation(region) {
        Some(abbreviation) => zone_id
            .to_ascii_lowercase()
            .strip_prefix(&abbreviation)
            .is_some_and(|rest| rest.starts_with('-')),
        None => true,
    }
}

/// Why the Availability Zone identifiers a request named are not usable in
/// `region`, or `None` when every one of them belongs to it.
///
/// An Availability Zone belongs to exactly one region (see
/// [`zone_name_in_region`]), so a request addressed to one region naming another
/// region's zone describes a resource that cannot exist — a bad parameter, which
/// this API answers as its `ValidationException` (400). AWS states that as
/// `<the value as supplied> does not match your region.`
///
/// A value that names THIS region and no zone within it is refused too, but it is
/// not a cross-region mismatch, so it is not worded as one.
///
/// The zone ID is reported first: it is the region-stable identifier of a zone (a
/// zone NAME is per-account), so when a request names both and neither belongs to
/// the region, the ID is the identifier the refusal names.
pub fn availability_zone_region_refusal(
    region: &str,
    zone: Option<&str>,
    zone_id: Option<&str>,
) -> Option<String> {
    zone_id
        .and_then(|zone_id| zone_id_refusal(zone_id, region))
        .or_else(|| zone.and_then(|zone| zone_name_refusal(zone, region)))
}

/// Why `zone` is not an Availability Zone name `region` can place a resource in,
/// or `None` when it is one of its zones.
fn zone_name_refusal(zone: &str, region: &str) -> Option<String> {
    if zone_name_in_region(zone, region) {
        return None;
    }
    Some(match zone.eq_ignore_ascii_case(region) {
        true => names_no_zone(zone, region),
        false => wrong_region(zone),
    })
}

/// Why `zone_id` is not an Availability Zone ID `region` can place a resource in,
/// or `None` when it is one of its zones.
fn zone_id_refusal(zone_id: &str, region: &str) -> Option<String> {
    if zone_id_in_region(zone_id, region) {
        return None;
    }
    let names_region = region_abbreviation(region)
        .is_some_and(|abbreviation| zone_id.eq_ignore_ascii_case(&abbreviation));
    Some(match names_region {
        true => names_no_zone(zone_id, region),
        false => wrong_region(zone_id),
    })
}

/// How this API words a zone of another region, as the recorded response does:
/// the value exactly as the request supplied it, and no more.
fn wrong_region(value: &str) -> String {
    format!("{value} does not match your region.")
}

/// How this service words a value that names the request's own region but no zone
/// within it — refused for the same reason (it is no zone of this region) but
/// stated separately, because calling it a region mismatch would be wrong.
fn names_no_zone(value: &str, region: &str) -> String {
    format!("{value} is not an Availability Zone of {region}.")
}

/// The `Site` projection of an operating address: the three members `Site`
/// declares for it, each read off the address the caller supplied.
pub fn operating_address_projection(
    address: Option<&Address>,
) -> (Option<String>, Option<String>, Option<String>) {
    match address {
        Some(address) => (
            Some(address.country_code.clone()),
            Some(address.state_or_region.clone()),
            Some(address.city.clone()),
        ),
        None => (None, None, None),
    }
}

/// A taggable resource's tag map as a response member: ALWAYS present, and empty
/// for a resource that carries no tags.
///
/// A resource's own tag map is part of it, so every projection of that resource
/// reports it — a resource with no tags has an empty map, not a missing member, and
/// a client that iterates the member gets an empty iteration rather than a null.
/// `TagMap`'s `length(min: 1)` constrains what a REQUEST may carry (the framework
/// validates input against the model before a handler runs); it does not turn a
/// resource's empty tag set into an absent member on the way out.
pub fn tags_member(tags: &HashMap<String, String>) -> Option<HashMap<String, String>> {
    Some(tags.clone())
}

/// How the Availability-Zone-ID region abbreviation renders each word an AWS region
/// code can carry, compound directions FIRST so `se` is read as `southeast` rather
/// than as `south` followed by `east`.
///
/// AWS writes an Availability Zone ID as `<region abbreviation>-az<n>`, where the
/// abbreviation is the region's two-letter area code, the initials of each word of
/// the rest of the region code, and the region number: the AWS documentation's
/// published examples are `use1-az1` (`us-east-1`), `usw2-az2` (`us-west-2`),
/// `apse2-az1` (`ap-southeast-2`) and `usgw1-az1` (`us-gov-west-1`). A word the
/// table does not name is abbreviated to its own first letter, which is the same
/// rule the published examples follow for `gov`.
const REGION_WORDS: &[(&str, &str)] = &[
    ("northeast", "ne"),
    ("northwest", "nw"),
    ("southeast", "se"),
    ("southwest", "sw"),
    ("central", "c"),
    ("north", "n"),
    ("south", "s"),
    ("east", "e"),
    ("west", "w"),
    ("gov", "g"),
    ("iso", "i"),
];

/// The highest `az<n>` index, and so the number of zone letters, this API can name:
/// the modelled `AvailabilityZoneId` pattern `^[a-zA-Z]+\d-[a-zA-Z]+\d$` ends in
/// exactly ONE digit.
const MAX_ZONE_INDEX: u32 = 9;

/// The Availability-Zone-ID abbreviation of an AWS region code (`us-west-2` ->
/// `usw2`), or `None` when the value is not a region code this rule can abbreviate
/// into the letters-then-one-digit prefix the modelled `AvailabilityZoneId` wants.
fn region_abbreviation(region: &str) -> Option<String> {
    let mut segments = region.split('-');
    let area = segments.next()?;
    if area.len() != 2 || !area.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    let mut words: Vec<&str> = segments.collect();
    let number = words.pop()?;
    if number.len() != 1 || !number.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let mut abbreviation = area.to_string();
    for word in words {
        abbreviation.push_str(abbreviate_region_word(word)?);
    }
    abbreviation.push_str(number);
    Some(abbreviation)
}

/// The letters [`region_abbreviation`] renders one word of a region code as. See
/// [`REGION_WORDS`].
fn abbreviate_region_word(word: &str) -> Option<&str> {
    if word.is_empty() || !word.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    REGION_WORDS
        .iter()
        .find(|(name, _)| *name == word)
        .map(|(_, initials)| *initials)
        .or_else(|| word.get(..1))
}

/// The region code an Availability-Zone-ID abbreviation names (`usw2` ->
/// `us-west-2`): the inverse of [`region_abbreviation`], so the two agree on every
/// region whose words [`REGION_WORDS`] names.
fn region_from_abbreviation(abbreviation: &str) -> Option<String> {
    if !abbreviation.is_ascii() || abbreviation.len() < 4 {
        return None;
    }
    let (letters, number) = abbreviation.split_at(abbreviation.len() - 1);
    if !number.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let (area, mut rest) = letters.split_at(2);
    if !area.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    let mut region = area.to_string();
    while !rest.is_empty() {
        let (word, initials) = REGION_WORDS
            .iter()
            .find(|(_, initials)| rest.starts_with(*initials))?;
        region.push('-');
        region.push_str(word);
        rest = &rest[initials.len()..];
    }
    region.push('-');
    region.push_str(number);
    Some(region)
}

/// The `az<n>` index the emulator pairs an Availability Zone's letter with.
///
/// AWS maps zone names to zone ids per account and publishes no formula for the
/// index, so this service fixes ONE stable bijection — `…a` <-> `…-az1`, `…b` <->
/// `…-az2`, and so on — and resolves in both directions with it, so a zone named
/// either way always resolves to the same pair.
fn zone_index(letter: u8) -> Option<u32> {
    let index = u32::from(letter.to_ascii_lowercase().checked_sub(b'a')?) + 1;
    (letter.is_ascii_alphabetic() && index <= MAX_ZONE_INDEX).then_some(index)
}

/// The Availability Zone letter an `az<n>` index names. The inverse of
/// [`zone_index`].
fn zone_letter(index: u32) -> Option<char> {
    (1..=MAX_ZONE_INDEX)
        .contains(&index)
        .then(|| char::from(b'a' + u8::try_from(index - 1).unwrap_or(0)))
}

/// The `AvailabilityZoneId` of the zone an `AvailabilityZone` name names
/// (`us-west-2b` -> `usw2-az2`), or `None` when the name carries no region and zone
/// letter to resolve one from.
pub fn availability_zone_id(zone: &str) -> Option<String> {
    if !zone.is_ascii() || zone.is_empty() {
        return None;
    }
    let (region, letter) = zone.split_at(zone.len() - 1);
    let index = zone_index(*letter.as_bytes().first()?)?;
    Some(format!("{}-az{index}", region_abbreviation(region)?))
}

/// The `AvailabilityZone` name of the zone an `AvailabilityZoneId` names
/// (`usw2-az2` -> `us-west-2b`): the inverse of [`availability_zone_id`].
pub fn availability_zone(zone_id: &str) -> Option<String> {
    if !zone_id.is_ascii() {
        return None;
    }
    let (abbreviation, index) = zone_id.split_once("-az")?;
    let letter = zone_letter(index.parse().ok()?)?;
    Some(format!("{}{letter}", region_from_abbreviation(abbreviation)?))
}

/// A string member the caller may have sent as the empty string, which carries no
/// value: every optional string in this API is modelled `length(min: 1)` except
/// `OutpostDescription`, so an empty one is omitted rather than stored.
pub fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

/// Apply one member of a partial (PATCH) update to the stored resource.
///
/// Every update in this API is modelled `@http(method: "PATCH")` with optional
/// members, so a member the request carries replaces the stored one and a member
/// it omits leaves the stored one exactly as it was. Shared by the three update
/// operations so they cannot drift apart on that rule.
pub fn apply<T>(stored: &mut Option<T>, update: Option<T>) {
    if let Some(value) = update {
        *stored = Some(value);
    }
}

// ---------------------------------------------------------------------------
// How this service words a refused request
//
// Two rules, each applied to every operation in this crate that states that
// class of failure, so no handler can word one its own way:
//
// * `NotFoundException` — the body is the IDENTIFIER ECHO
//   `<member>=<value as supplied>`. See [`identifier_not_found`].
// * `ValidationException` — the body is the standard AWS validation report, and
//   it never renders the offending value. See [`validation_report`].
// ---------------------------------------------------------------------------

/// How this service states that an identifier names no resource it holds.
///
/// The body is the identifier ECHO: the name of the input member the identifier
/// arrived in, `=`, and the value EXACTLY as the client sent it — so a caller
/// that sent the ARN form reads its ARN back, not the bare id the service
/// resolved it to, and a caller can parse the identifier it sent out of the
/// message. It is not an English sentence, and it never normalises the value.
///
/// `member` is the member's validation-path name (see [`SITE_ID_MEMBER`]);
/// `supplied` is the raw input member, before [`resolve_id`] touches it.
pub fn identifier_not_found(member: &str, supplied: &str) -> String {
    format!("{member}={supplied}")
}

/// The validation-path name of the `SiteId` input member.
///
/// Both message rules name a member the way AWS's validation report names one:
/// lowerCamel, which is the report's own casing convention applied to the Smithy
/// member name (`SiteId` -> `siteId`), not a fact of the model.
pub const SITE_ID_MEMBER: &str = "siteId";

/// The validation-path name of the `OutpostId` input member. See [`SITE_ID_MEMBER`].
pub const OUTPOST_ID_MEMBER: &str = "outpostId";

/// The validation-path name of the `CapacityTaskId` input member. See
/// [`SITE_ID_MEMBER`].
pub const CAPACITY_TASK_ID_MEMBER: &str = "capacityTaskId";

/// The validation-path name of the `NextToken` input member every list takes.
/// See [`SITE_ID_MEMBER`].
pub const NEXT_TOKEN_MEMBER: &str = "nextToken";

/// The validation-path name of the `QuoteIdentifier` input member the quote
/// operations bind. See [`SITE_ID_MEMBER`].
pub const QUOTE_IDENTIFIER_MEMBER: &str = "quoteIdentifier";

/// The validation-path name of the `OutpostIdentifier` input member the quote and
/// capacity-task operations take: a DIFFERENT member from the `OutpostId` the
/// Outpost operations bind, so a refusal about it names it as the request spelled
/// it. See [`SITE_ID_MEMBER`].
pub const OUTPOST_IDENTIFIER_MEMBER: &str = "outpostIdentifier";

/// The `@pattern` the model declares on `SiteId`, verbatim: a report names a
/// pattern exactly as the model spells it, anchors included.
pub const SITE_ID_PATTERN: &str =
    r"^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:site/)?(os-[a-f0-9]{17})$";

/// The `@pattern` the model declares on `OutpostId`. See [`SITE_ID_PATTERN`].
pub const OUTPOST_ID_PATTERN: &str =
    r"^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:outpost/)?op-[a-f0-9]{17}$";

/// The `@pattern` the model declares on `QuoteIdentifier`. See
/// [`SITE_ID_PATTERN`].
pub const QUOTE_IDENTIFIER_PATTERN: &str =
    r"^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:quote/)?oq-[a-f0-9]{17}$";

/// The `@pattern` the model declares on `OutpostIdentifier`, the member the quote
/// and capacity-task operations name an Outpost by. See [`SITE_ID_PATTERN`].
pub const OUTPOST_IDENTIFIER_PATTERN: &str =
    r"^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:outpost/)?op-[a-f0-9]{17}$";

/// The error code every refusal this service rewords answers with — the same
/// code the modelled `ValidationException` (HTTP 400) carries, so rewording the
/// body never moves a client off the code it branches on.
pub const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The text [`Violation::message`] puts between the offending value's position
/// and the requirement it failed.
///
/// The LAST occurrence is the real separator: a requirement never contains this
/// text, while a caller-supplied value interpolated before it might.
const CONSTRAINT_SEPARATOR: &str = " failed to satisfy constraint: ";

/// One clause of the validation report: WHERE the offending value was, and WHAT
/// it had to satisfy. The value itself is never part of a clause.
pub fn clause(member: &str, requirement: &str) -> String {
    format!("Value at '{member}' failed to satisfy constraint: {requirement}")
}

/// The standard AWS validation report around `clauses`: the count, then every
/// clause joined with `; `.
///
/// Callers pass at least one clause — a report with nothing to report is not a
/// refusal — and the plural arm renders whatever it is given rather than
/// panicking on an empty slice.
pub fn validation_report(clauses: &[String]) -> String {
    match clauses {
        [single] => format!("1 validation error detected: {single}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// The clause a single [`Violation`] states under this service's rule.
///
/// The requirement is lifted out of the framework's OWN rendering of the
/// violation instead of being re-spelled here, so a constraint this service
/// reports is worded exactly as the boundary validator words it — the same
/// bound text, and a `@pattern` reproduced verbatim from the model. What this
/// drops is the rendering of the value: the report names the member and the
/// constraint only.
fn violation_clause(violation: &Violation) -> String {
    let rendered = violation.message();
    match rendered.rsplit_once(CONSTRAINT_SEPARATOR) {
        // A member that was ABSENT has no value to omit, and the framework's own
        // clause already says exactly that (`Value null at '<member>' …`), so it
        // stands unchanged.
        Some((_, requirement)) if violation.value.is_some() => {
            clause(&violation.path, requirement)
        }
        _ => rendered,
    }
}

/// The report for every constraint violation the boundary validator found,
/// in the order it found them. This is what `override_refusal` answers with.
pub fn refusal_report(violations: &[Violation]) -> String {
    let clauses: Vec<String> = violations.iter().map(violation_clause).collect();
    validation_report(&clauses)
}

/// The report for one member that failed `kind`, for a handler that has to state
/// a modelled constraint itself.
pub fn constraint_report(member: &str, kind: ViolationKind) -> String {
    validation_report(&[violation_clause(&Violation {
        path: member.to_string(),
        kind,
        // A PRESENT value selects the value-omitting clause rather than the
        // `Value null` one an absent member gets. What it holds is irrelevant:
        // the value never reaches the body.
        value: Some(String::new()),
    })])
}

/// The report a `SiteId` that is neither a site id nor a site ARN states.
pub fn site_id_pattern_report() -> String {
    constraint_report(
        SITE_ID_MEMBER,
        ViolationKind::Pattern(SITE_ID_PATTERN.to_string()),
    )
}

/// The report an `OutpostId` that is neither an Outpost id nor an Outpost ARN
/// states. See [`site_id_pattern_report`].
pub fn outpost_id_pattern_report() -> String {
    constraint_report(
        OUTPOST_ID_MEMBER,
        ViolationKind::Pattern(OUTPOST_ID_PATTERN.to_string()),
    )
}

/// How this service states that an Outpost (or an order) already has an active
/// capacity task, and so cannot take another.
///
/// AWS publishes no wording for this refusal, so the sentence is the emulator's
/// own; what the caller branches on is the `ConflictException` code (HTTP 409) and
/// the `ResourceId` / `ResourceType` members the model puts on it, which name the
/// resource the conflict is about. The message names the task already holding it,
/// so the caller can poll exactly that one.
pub fn capacity_task_conflict(resource_type: &str, resource_id: &str, active: &str) -> String {
    format!("{resource_type} {resource_id} already has an active capacity task: {active}")
}

/// The `ResourceType` a capacity-task conflict on an Outpost names, from the
/// modelled `ResourceType` enum (`ORDER | OUTPOST`).
pub const RESOURCE_TYPE_OUTPOST: &str = "OUTPOST";

/// The `ResourceType` a capacity-task conflict on an order names. See
/// [`RESOURCE_TYPE_OUTPOST`].
pub const RESOURCE_TYPE_ORDER: &str = "ORDER";

/// The report a `QuoteIdentifier` that is neither a quote id nor a quote ARN
/// states. See [`site_id_pattern_report`].
pub fn quote_identifier_pattern_report() -> String {
    constraint_report(
        QUOTE_IDENTIFIER_MEMBER,
        ViolationKind::Pattern(QUOTE_IDENTIFIER_PATTERN.to_string()),
    )
}

/// The report an `OutpostIdentifier` that is neither an Outpost id nor an Outpost
/// ARN states. See [`site_id_pattern_report`].
pub fn outpost_identifier_pattern_report() -> String {
    constraint_report(
        OUTPOST_IDENTIFIER_MEMBER,
        ViolationKind::Pattern(OUTPOST_IDENTIFIER_PATTERN.to_string()),
    )
}

/// The report an unusable pagination token states.
///
/// No modelled constraint covers it — the modelled `Token` pattern has already
/// passed by the time the token is decoded — so the requirement is this
/// service's own rule, stated in the report's grammar and naming `operation`,
/// the list the token had to belong to.
pub fn page_token_report(operation: &str) -> String {
    validation_report(&[clause(
        NEXT_TOKEN_MEMBER,
        &format!("Member must be a pagination token this service issued for {operation}"),
    )])
}

/// The stamp a `ListOutposts` page token carries, so a token issued for one list
/// cannot be replayed against another.
pub const OUTPOST_PAGE: &str = "outposts";

/// The stamp a `ListSites` page token carries. See [`OUTPOST_PAGE`].
pub const SITE_PAGE: &str = "sites";

/// The page size a list uses when the caller named no `MaxResults`, which is also
/// the ceiling the model declares for it (`MaxResults1000`, `range(1..1000)`).
pub const MAX_RESULTS: i32 = 1000;

/// The ceiling on how many sites, how many Outposts, how many quotes, and how many
/// capacity tasks this store holds.
///
/// A generated emulator keeps its resources in memory for the life of the process
/// and a create is the only thing that adds one, so the store needs a maximum entry
/// count: neither a TTL nor an eviction policy is available to it, because `GetSite`
/// must still answer what `CreateSite` created, and `GetCapacityTask` must still
/// answer a task that has long since completed. Past the cap the creating operation
/// refuses the request rather than growing: the two creates that model
/// `ServiceQuotaExceededException` (HTTP 402) answer it (it is modelled on exactly
/// them and on no read), `CreateQuote`, whose model declares no quota error, refuses
/// the create as a bad request (400 `ValidationException`) rather than inventing an
/// error the model does not carry, and `StartCapacityTask`, which models no quota
/// error either, answers the `ConflictException` (HTTP 409) it does model, worded by
/// [`store_full`] so the caller can tell it from the one-active-task refusal. The
/// number is the emulator's own bound, not a published AWS default (AWS publishes
/// these quotas as adjustable, so no single value is the contract), and is set high
/// enough that no ordinary workflow meets it.
pub const MAX_RESOURCES: usize = 5_000;

/// How this service states that a store has reached [`MAX_RESOURCES`] and cannot
/// take another resource of that kind.
///
/// The ceiling is the emulator's own, so no AWS rule fixes this wording; what the
/// caller branches on is the modelled code and status. The message names the
/// emulator, the kind of resource and the bound, so the refusal cannot be mistaken
/// for a statement about the caller's request or about a real AWS quota.
pub fn store_full(resource: &str) -> String {
    format!(
        "this emulator holds the maximum of {MAX_RESOURCES} {resource} it can keep in \
         memory, so it cannot take another"
    )
}

/// Encode a page boundary as an opaque `Token`.
///
/// The token carries the creation sequence of the LAST item the page returned, so
/// the next page is everything created strictly after it — a resume point that
/// stays correct when items are added or removed between pages, which a positional
/// offset would not. It is STAMPED with the list it belongs to, so a token replayed
/// against another list is refused rather than honoured as that list's boundary.
/// The rendering is exactly the modelled `Token` pattern `^(\d+)##(\S+)$`.
pub fn encode_page_token(kind: &str, last_sequence: u64) -> String {
    format!("{last_sequence}##{kind}")
}

/// The page boundary a `kind` token names, or `None` when it is not a token this
/// service issued for that list — which the caller is told as a
/// `ValidationException`, never as a silently truncated page.
pub fn decode_page_token(kind: &str, token: &str) -> Option<u64> {
    let (sequence, stamp) = token.split_once("##")?;
    if stamp != kind {
        return None;
    }
    sequence.parse().ok()
}

/// The page size a list serves, given the `MaxResults` the caller sent.
///
/// `MaxResults1000` is modelled `range(1..1000)`, so the framework has already
/// refused anything outside it; an absent value means the whole list up to that
/// same ceiling.
pub fn page_size(max_results: Option<i32>) -> usize {
    usize::try_from(max_results.unwrap_or(MAX_RESULTS))
        .unwrap_or(MAX_RESULTS as usize)
        .clamp(1, MAX_RESULTS as usize)
}

/// Cut one page out of an already-filtered sequence of records, and issue the token
/// that resumes after it.
///
/// Shared by both list operations so they cannot drift apart: the records are
/// ordered by creation sequence, one more than the page would fill is what proves a
/// further page exists, and the token names the last sequence this page returned.
pub fn page<T>(
    kind: &str,
    records: impl Iterator<Item = (u64, T)>,
    after: Option<u64>,
    size: usize,
) -> (Vec<T>, Option<String>) {
    let mut ordered: Vec<(u64, T)> = records
        .filter(|(sequence, _)| after.is_none_or(|boundary| *sequence > boundary))
        .collect();
    ordered.sort_by_key(|(sequence, _)| *sequence);
    let next_token = match ordered.len() > size {
        true => {
            ordered.truncate(size);
            ordered
                .last()
                .map(|(sequence, _)| encode_page_token(kind, *sequence))
        }
        false => None,
    };
    (
        ordered.into_iter().map(|(_, item)| item).collect(),
        next_token,
    )
}
