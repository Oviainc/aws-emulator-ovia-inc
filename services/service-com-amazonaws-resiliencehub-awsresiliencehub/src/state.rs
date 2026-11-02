// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-resiliencehub-awsresiliencehub service.

use crate::ops::types::{
    App, AppComponent, FailurePolicy, LogicalResourceId, PhysicalResource, ResiliencyPolicy,
    ESTIMATED_COST_TIER_VALUES,
};
use std::collections::{BTreeMap, HashMap, VecDeque};

/// The region every minted ARN falls back to when the request carries no usable
/// SigV4 credential scope (an unsigned, in-process call, say).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `appVersion` of an application's DRAFT version — the one version every
/// application has, and the only one the `CreateAppVersion*` / `UpdateAppVersion*`
/// operations write to (`PublishAppVersion` is what freezes a snapshot of it as
/// the current release).
pub const DRAFT_APP_VERSION: &str = "draft";

/// The `AppVersionSummary$identifier` of an application's DRAFT version: 0.
///
/// Every listed application version answers `identifier` — it is the only ordering
/// key a client has over versions, the `appVersion` label being an opaque string —
/// and the draft is the version every application starts from, so it sits below
/// every publish sequence number.
pub const DRAFT_APP_VERSION_IDENTIFIER: i64 = 0;

/// The `appVersion` that addresses an application's CURRENT RELEASE — the newest
/// snapshot `PublishAppVersion` froze from the draft.
///
/// A FIXED alias, not a number: the version LABEL and the publish sequence number
/// are distinct values in the model (`PublishAppVersion` answers both `appVersion`,
/// an `EntityVersion`, and `identifier`, a `LongOptional`), and this alias is the
/// `appVersion` every reader of a published version — `DescribeAppVersionTemplate`,
/// `ListAppVersionResources`, `DescribeAppVersionResource`,
/// `ResolveAppVersionResources` — addresses the current release by. A re-publish
/// replaces what it resolves to; see
/// [`ComAmazonawsResiliencehubAwsresiliencehubState::publish_app_version`].
pub const RELEASE_APP_VERSION: &str = "release";

/// The most applications this emulator stores.
///
/// A MEMORY BOUND, not an AWS quota: `CreateApp` is reachable by any caller and
/// stores a caller-sized record (description, tags, event subscriptions) per call,
/// so without a ceiling a request loop grows the process until the host is out of
/// memory. Past it `CreateApp` answers its own modelled
/// `ServiceQuotaExceededException` (HTTP 402) — the channel AWS uses when an
/// application cannot be created because a limit is reached.
pub const MAX_APPS: usize = 200;

/// The most resiliency policies this emulator stores, for the same reason as
/// [`MAX_APPS`]. `CreateResiliencyPolicy` answers `ServiceQuotaExceededException`
/// past it.
pub const MAX_RESILIENCY_POLICIES: usize = 200;

/// The most PUBLISHED app versions this emulator stores for ONE application.
///
/// A MEMORY BOUND, not an AWS quota: `PublishAppVersion` is reachable by any
/// caller and freezes a COPY of the draft's contents per call, so without a
/// ceiling a publish loop grows the process until the host is out of memory.
/// `PublishAppVersion` models no `ServiceQuotaExceededException`, so past the
/// ceiling it answers its own modelled `ValidationException` (HTTP 400) — the
/// channel its model offers for a request this service will not carry out —
/// naming the ceiling.
pub const MAX_PUBLISHED_APP_VERSIONS_PER_APP: usize = 200;

/// The most Application Components this emulator stores in ONE app version.
///
/// A MEMORY BOUND, not an AWS quota, for the same reason as [`MAX_APPS`]:
/// `CreateAppVersionAppComponent` is reachable by any caller and stores a
/// caller-sized record per call. Past it the operation answers its own modelled
/// `ServiceQuotaExceededException` (HTTP 402) — the channel AWS uses when a
/// resource cannot be created because a limit is reached.
pub const MAX_APP_COMPONENTS_PER_APP_VERSION: usize = 200;

/// The most physical resources this emulator stores in ONE app version.
///
/// A MEMORY BOUND, not an AWS quota, for the same reason as [`MAX_APPS`]:
/// `CreateAppVersionResource` is reachable by any caller and stores a
/// caller-sized record per call. Past it the operation answers its own modelled
/// `ServiceQuotaExceededException` (HTTP 402) — the channel AWS uses when a
/// resource cannot be created because a limit is reached.
pub const MAX_RESOURCES_PER_APP_VERSION: usize = 200;

/// The most resource resolutions this emulator remembers for ONE app version,
/// oldest evicted first.
///
/// A MEMORY BOUND, not an AWS quota: `ResolveAppVersionResources` is reachable by
/// any caller and records one resolution per call, so without a ceiling a resolve
/// loop grows the process until the host is out of memory. Evicting the oldest
/// rather than refusing the call is what AWS does with resolution history — the
/// CURRENT resolution of a version is always readable — and a resolution the
/// emulator has forgotten is reported as the modelled 404 by
/// `DescribeAppVersionResourcesResolutionStatus`. Enforced in
/// [`ComAmazonawsResiliencehubAwsresiliencehubState::start_resolution`], the only
/// writer.
pub const MAX_RESOLUTIONS_PER_APP_VERSION: usize = 50;

/// How long a resource resolution started by `ResolveAppVersionResources` takes
/// to finish.
///
/// CHOSEN, not a documented AWS duration: a real resolution's duration depends on
/// how many resources it walks. It is never zero, so a caller that reads the
/// resolution straight after starting it still sees the non-terminal status.
pub const RESOLUTION_SPAN: ::std::time::Duration = ::std::time::Duration::from_secs(3);

/// The `ResourceResolutionStatusType` value a resolution that has not finished
/// yet reports — the non-terminal status `ResolveAppVersionResources` answers
/// with (one of `RESOURCE_RESOLUTION_STATUS_TYPE_VALUES`, the enum the model
/// gives `ResolveAppVersionResources$status`).
pub const RESOLUTION_STATUS_PENDING: &str = "Pending";

/// The `ResourceResolutionStatusType` value a finished resolution reports — the
/// terminal status a read finds once [`RESOLUTION_SPAN`] has elapsed.
pub const RESOLUTION_STATUS_SUCCESS: &str = "Success";

/// The modelled `@length` of `AdditionalInfoValueList` — the value list of every
/// `AdditionalInfoMap` entry — transcribed from the model (`smithy_query
/// query="shape" shape="AdditionalInfoMap"`: `value` targets
/// `AdditionalInfoValueList`, `length` 1..10). Enforced by
/// [`additional_info_violation`].
const ADDITIONAL_INFO_VALUES_MIN: usize = 1;

/// The upper half of that bound; see [`ADDITIONAL_INFO_VALUES_MIN`].
const ADDITIONAL_INFO_VALUES_MAX: usize = 10;

/// Hard ceiling on remembered idempotency tokens, oldest evicted first.
///
/// A BACKSTOP. One token is recorded per successfully created resource and
/// `DeleteApp` forgets the tokens of the app it removes, so the map is already
/// bounded by [`MAX_APPS`] + [`MAX_RESILIENCY_POLICIES`] while every creating
/// handler releases what it allocated. The ceiling is here so that invariant does
/// not depend on a future handler remembering to, and is enforced in
/// [`ComAmazonawsResiliencehubAwsresiliencehubState::record_idempotency_token`],
/// the only writer.
pub const MAX_IDEMPOTENCY_TOKENS: usize = MAX_APPS + MAX_RESILIENCY_POLICIES;

/// What one app version of one application holds.
///
/// The modelled shapes ARE the stored records, exactly as for `App` and
/// `ResiliencyPolicy`: a Describe is a clone of the stored value, so a read can
/// never disagree with what the last write persisted.
#[derive(Debug, Default, Clone)]
pub struct AppVersionContents {
    /// The version's Application Components, keyed by their `id`. A `BTreeMap`
    /// because a reader (`ListAppVersionAppComponents`, and any paginated read
    /// built on it) needs one stable order across calls. At most
    /// [`MAX_APP_COMPONENTS_PER_APP_VERSION`] entries.
    pub app_components: BTreeMap<String, AppComponent>,
    /// The version's physical resources, keyed by [`logical_resource_key`] of the
    /// `logicalResourceId` they were created with — the member the model makes
    /// REQUIRED on both `CreateAppVersionResource`'s input and `PhysicalResource`,
    /// and so the one that identifies a resource within one app version. A
    /// `BTreeMap` for the same reason `app_components` is one: a reader needs one
    /// stable order across calls. At most [`MAX_RESOURCES_PER_APP_VERSION`]
    /// entries.
    pub resources: BTreeMap<String, PhysicalResource>,
    /// The `versionName` the `PublishAppVersion` call that minted this version
    /// carried, when it named one — the modelled `AppVersionSummary$versionName`.
    /// Always `None` for the draft, which nobody names.
    pub version_name: Option<String>,
    /// The modelled `AppVersionSummary$identifier` of this version: the publish
    /// sequence number a published version was minted with. `None` for the draft,
    /// which is not a published version and so has no identifier.
    pub identifier: Option<i64>,
    /// When this version was published — the modelled
    /// `AppVersionSummary$creationTime`. `None` for the draft, which comes into
    /// being with the application and whose creation time is the application's own
    /// `creationTime`.
    pub creation_time: Option<::aws_smithy_types::DateTime>,
}

impl AppVersionContents {
    /// Whether this app version holds nothing — the question
    /// `PublishAppVersion` asks of a draft before freezing it.
    ///
    /// Every member a draft writer persists is counted here, so adding one (a
    /// resource, an app template body) means adding it to this test rather than
    /// to each caller of it.
    pub fn is_empty(&self) -> bool {
        self.app_components.is_empty() && self.resources.is_empty()
    }

    /// The resource this version holds under `logical` — the exact
    /// `logicalResourceId` it was created with.
    pub fn resource_by_logical_id(&self, logical: &LogicalResourceId) -> Option<&PhysicalResource> {
        self.resources.get(&logical_resource_key(logical))
    }

    /// The resource this version holds under the name `resource_name`, in the
    /// map's stable order.
    pub fn resource_by_name(&self, resource_name: &str) -> Option<&PhysicalResource> {
        self.resources
            .values()
            .find(|resource| resource.resource_name.as_deref() == Some(resource_name))
    }

    /// The resource this version holds for the physical identifier `identifier`,
    /// narrowed to `aws_region` and `aws_account_id` when the caller named them —
    /// the three members `DescribeAppVersionResource` addresses a resource by its
    /// physical id with.
    pub fn resource_by_physical_id(
        &self,
        identifier: &str,
        aws_region: Option<&str>,
        aws_account_id: Option<&str>,
    ) -> Option<&PhysicalResource> {
        self.resources.values().find(|resource| {
            let physical = &resource.physical_resource_id;
            physical.identifier == identifier
                && aws_region.is_none_or(|region| physical.aws_region.as_deref() == Some(region))
                && aws_account_id
                    .is_none_or(|account| physical.aws_account_id.as_deref() == Some(account))
        })
    }

    /// The Application Component of this version the caller names, by the `name`
    /// the model gives `AppComponentNameList`'s members or by the `id` it is
    /// stored under.
    pub fn app_component_named(&self, name: &str) -> Option<&AppComponent> {
        self.app_components
            .values()
            .find(|component| component.name == name || component.id.as_deref() == Some(name))
    }
}

/// One resource resolution of one app version: the record
/// `ResolveAppVersionResources` starts and
/// `DescribeAppVersionResourcesResolutionStatus` reports.
///
/// The STATUS is not stored: it is derived from the clock in [`Self::status`], so
/// a read never has to write to move a resolution on and two readers at the same
/// instant cannot disagree about it.
#[derive(Debug, Clone)]
pub struct ResourceResolution {
    /// The server-generated `resolutionId` this resolution is addressed by —
    /// unique within this emulator, and never derived from the request.
    pub resolution_id: String,
    /// When this resolution finishes: the instant the clock owes its move from
    /// the non-terminal status to the terminal one, whether or not anybody asks.
    pub completes_at: ::std::time::SystemTime,
}

impl ResourceResolution {
    /// A resolution started at `now`, finishing one [`RESOLUTION_SPAN`] later.
    pub fn started_at(now: ::std::time::SystemTime) -> Self {
        Self {
            resolution_id: mint_resource_id(),
            completes_at: now + RESOLUTION_SPAN,
        }
    }

    /// This resolution's `ResourceResolutionStatusType` at `now`:
    /// [`RESOLUTION_STATUS_SUCCESS`] once the clock has reached
    /// [`Self::completes_at`], [`RESOLUTION_STATUS_PENDING`] until then.
    pub fn status(&self, now: ::std::time::SystemTime) -> &'static str {
        if now >= self.completes_at {
            RESOLUTION_STATUS_SUCCESS
        } else {
            RESOLUTION_STATUS_PENDING
        }
    }
}

/// Per-service state for com-amazonaws-resiliencehub-awsresiliencehub. A handler reaches it through [`service_state`].
///
/// The modelled `App` and `ResiliencyPolicy` shapes ARE the stored records: a
/// Describe is a clone of the stored value, so a read can never disagree with what
/// the last write persisted.
///
/// Every map here is caller-driven, so every one is bounded: `apps` by
/// [`MAX_APPS`], `app_name_index` by `apps` (one entry per stored app),
/// `policies` by [`MAX_RESILIENCY_POLICIES`], `app_versions` by `apps` (an entry
/// exists only for a stored application, and `DeleteApp` drops it) times
/// [`MAX_PUBLISHED_APP_VERSIONS_PER_APP`] plus the draft, times
/// [`MAX_APP_COMPONENTS_PER_APP_VERSION`] per version, the idempotency store
/// by [`MAX_IDEMPOTENCY_TOKENS`], and `resolutions` by the same app versions
/// times [`MAX_RESOLUTIONS_PER_APP_VERSION`].
#[derive(Debug, Default)]
pub struct ComAmazonawsResiliencehubAwsresiliencehubState {
    /// Every stored application, keyed by its minted `appArn`. At most
    /// [`MAX_APPS`] entries.
    pub apps: HashMap<String, App>,
    /// `name` -> `appArn`. Application names are unique within an account, so this
    /// is both the name filter's index and the duplicate-name conflict detector.
    /// Exactly one entry per entry of `apps`.
    pub app_name_index: HashMap<String, String>,
    /// Every stored resiliency policy, keyed by its minted `policyArn`. At most
    /// [`MAX_RESILIENCY_POLICIES`] entries.
    pub policies: HashMap<String, ResiliencyPolicy>,
    /// What each app version of each application holds: `appArn` ->
    /// `appVersion` -> the version's contents. An entry is created only for a
    /// stored application and dropped by `DeleteApp`, so this map never outlives
    /// `apps`.
    ///
    /// The DRAFT version of a stored application exists whether or not it has an
    /// entry here — [`Self::app_version_exists`] is what answers that — so an
    /// application nobody has added anything to costs nothing.
    pub app_versions: HashMap<String, HashMap<String, AppVersionContents>>,
    /// `<operation>/<clientToken>` -> the ARN that token's FIRST call created, so a
    /// replayed `@idempotencyToken` returns the first result instead of creating a
    /// second resource. Keyed by operation as well as token because the same token
    /// may legitimately be replayed to two different operations.
    ///
    /// PRIVATE, with `record_idempotency_token` / `created_by_token` /
    /// `forget_idempotency_tokens_for` as the only access: the entry-count ceiling
    /// cannot be bypassed by a handler inserting directly.
    idempotency_tokens: HashMap<String, String>,
    /// The keys of `idempotency_tokens` in the order they were first recorded, so
    /// the ceiling evicts the oldest.
    idempotency_order: VecDeque<String>,
    /// The resource resolutions of each app version, oldest first: `appArn` ->
    /// `appVersion` -> that version's resolutions. An entry is recorded only for
    /// a stored application's existing version and dropped by
    /// [`Self::forget_app_versions`], so this map never outlives `apps`.
    ///
    /// PRIVATE, with [`Self::start_resolution`] / [`Self::resolution`] /
    /// [`Self::current_resolution`] / [`Self::resolution_mut`] as the only
    /// access: the per-version ceiling cannot be bypassed by a handler inserting
    /// directly.
    resolutions: HashMap<String, HashMap<String, VecDeque<ResourceResolution>>>,
}

impl ComAmazonawsResiliencehubAwsresiliencehubState {
    /// Whether `app_version` of `app_arn` exists.
    ///
    /// The draft version exists for every stored application — it is created with
    /// the application and cannot be removed — while any other version exists only
    /// once something recorded it.
    pub fn app_version_exists(&self, app_arn: &str, app_version: &str) -> bool {
        if app_version == DRAFT_APP_VERSION {
            return self.apps.contains_key(app_arn);
        }
        self.app_versions
            .get(app_arn)
            .is_some_and(|versions| versions.contains_key(app_version))
    }

    /// What `app_version` of `app_arn` holds, when anything has been stored in it.
    ///
    /// `None` covers both "no such version" and "an empty draft": a reader that has
    /// to tell them apart asks [`Self::app_version_exists`] first.
    pub fn app_version(&self, app_arn: &str, app_version: &str) -> Option<&AppVersionContents> {
        self.app_versions
            .get(app_arn)
            .and_then(|versions| versions.get(app_version))
    }

    /// The draft version of `app_arn`, for writing — materialised empty on the
    /// first write to it. The caller has already established that `app_arn` names
    /// a stored application.
    pub fn draft_app_version_mut(&mut self, app_arn: &str) -> &mut AppVersionContents {
        self.app_versions
            .entry(app_arn.to_string())
            .or_default()
            .entry(DRAFT_APP_VERSION.to_string())
            .or_default()
    }

    /// How many PUBLISHED versions `app_arn` has: every stored version except the
    /// draft, which is not published.
    pub fn published_app_version_count(&self, app_arn: &str) -> usize {
        self.app_versions
            .get(app_arn)
            .map(|versions| {
                versions
                    .keys()
                    .filter(|version| version.as_str() != DRAFT_APP_VERSION)
                    .count()
            })
            .unwrap_or_default()
    }

    /// The publish sequence number the next published version of `app_arn` takes:
    /// one past the highest it already has, so the first publish is 1 and the
    /// numbers never repeat for one application.
    ///
    /// Derived from the stored versions rather than from a counter, so the number
    /// a version was minted with is the number a reader finds it under.
    pub fn next_published_app_version(&self, app_arn: &str) -> i64 {
        let highest = self
            .app_versions
            .get(app_arn)
            .and_then(|versions| {
                versions
                    .values()
                    .filter_map(|contents| contents.identifier)
                    .max()
            })
            .unwrap_or(0);
        highest.saturating_add(1)
    }

    /// Store `contents` as the new CURRENT RELEASE of `app_arn`, answering the
    /// `appVersion` it is addressed by — [`RELEASE_APP_VERSION`].
    ///
    /// The snapshot the alias resolved to until now keeps its contents under its
    /// OWN publish sequence number, so a superseded release stays readable by the
    /// number it was minted with while the alias always names the newest one.
    /// `contents.identifier` is that number for the snapshot being stored.
    ///
    /// What moves with the superseded snapshot is everything recorded ABOUT that
    /// version: its contents and the resolutions recorded against it, so a
    /// resolution of the release a client resolved before re-publishing stays with
    /// the snapshot it resolved and the new release starts with none of its own.
    ///
    /// The caller has already established that `app_arn` names a stored
    /// application.
    pub fn publish_app_version(
        &mut self,
        app_arn: &str,
        contents: AppVersionContents,
    ) -> &'static str {
        let versions = self.app_versions.entry(app_arn.to_string()).or_default();

        // The key the snapshot being superseded keeps, read BEFORE it is taken out,
        // so nothing is ever removed without somewhere to put it. Only a published
        // snapshot is stored under the alias and every one of those carries the
        // sequence number it was minted with, so the key is always there.
        let superseded_key = versions
            .get(RELEASE_APP_VERSION)
            .and_then(|superseded| superseded.identifier)
            .map(|identifier| identifier.to_string());
        if let Some(key) = superseded_key {
            if let Some(superseded) = versions.remove(RELEASE_APP_VERSION) {
                versions.insert(key.clone(), superseded);
            }
            if let Some(resolutions) = self.resolutions.get_mut(app_arn) {
                if let Some(recorded) = resolutions.remove(RELEASE_APP_VERSION) {
                    resolutions.insert(key, recorded);
                }
            }
        }

        let versions = self.app_versions.entry(app_arn.to_string()).or_default();
        versions.insert(RELEASE_APP_VERSION.to_string(), contents);
        RELEASE_APP_VERSION
    }

    /// Forget every app version of `app_arn`, so deleting an application releases
    /// what its versions allocated — their contents and their resolutions alike.
    pub fn forget_app_versions(&mut self, app_arn: &str) {
        self.app_versions.remove(app_arn);
        self.resolutions.remove(app_arn);
    }

    /// Record a resolution of `app_version` of `app_arn`, started at `now`, as
    /// that version's CURRENT resolution, and answer it. The caller has already
    /// established that `app_arn` names a stored application and that
    /// `app_version` is one of its versions.
    ///
    /// Past [`MAX_RESOLUTIONS_PER_APP_VERSION`] the oldest resolution of that
    /// version is forgotten, so a resolve loop cannot grow this map without
    /// bound.
    pub fn start_resolution(
        &mut self,
        app_arn: &str,
        app_version: &str,
        now: ::std::time::SystemTime,
    ) -> ResourceResolution {
        let resolution = ResourceResolution::started_at(now);
        let version_resolutions = self
            .resolutions
            .entry(app_arn.to_string())
            .or_default()
            .entry(app_version.to_string())
            .or_default();
        version_resolutions.push_back(resolution.clone());
        while version_resolutions.len() > MAX_RESOLUTIONS_PER_APP_VERSION {
            version_resolutions.pop_front();
        }
        resolution
    }

    /// The resolution of `app_version` of `app_arn` that `resolution_id`
    /// addresses, while it is still remembered.
    pub fn resolution(
        &self,
        app_arn: &str,
        app_version: &str,
        resolution_id: &str,
    ) -> Option<&ResourceResolution> {
        self.version_resolutions(app_arn, app_version)?
            .iter()
            .find(|resolution| resolution.resolution_id == resolution_id)
    }

    /// The CURRENT resolution of `app_version` of `app_arn` — the one most
    /// recently started — when that version has been resolved at all.
    pub fn current_resolution(
        &self,
        app_arn: &str,
        app_version: &str,
    ) -> Option<&ResourceResolution> {
        self.version_resolutions(app_arn, app_version)?.back()
    }

    /// The resolution `resolution_id` addresses, for writing.
    pub fn resolution_mut(
        &mut self,
        app_arn: &str,
        app_version: &str,
        resolution_id: &str,
    ) -> Option<&mut ResourceResolution> {
        self.resolutions
            .get_mut(app_arn)?
            .get_mut(app_version)?
            .iter_mut()
            .find(|resolution| resolution.resolution_id == resolution_id)
    }

    /// Every remembered resolution of `app_version` of `app_arn`, oldest first.
    fn version_resolutions(
        &self,
        app_arn: &str,
        app_version: &str,
    ) -> Option<&VecDeque<ResourceResolution>> {
        self.resolutions
            .get(app_arn)
            .and_then(|versions| versions.get(app_version))
    }

    /// The ARN `operation`'s `client_token` created on its first call, if it is
    /// still remembered.
    pub fn created_by_token(&self, operation: &str, client_token: &str) -> Option<&String> {
        self.idempotency_tokens
            .get(&idempotency_key(operation, client_token))
    }

    /// Remember that `operation`'s `client_token` created `arn`, evicting the
    /// oldest remembered token once [`MAX_IDEMPOTENCY_TOKENS`] is reached. Re-recording
    /// a key already present replaces its ARN and keeps its original position, so a
    /// replay cannot be used to push other callers' tokens out.
    pub fn record_idempotency_token(&mut self, operation: &str, client_token: &str, arn: &str) {
        let key = idempotency_key(operation, client_token);
        if self
            .idempotency_tokens
            .insert(key.clone(), arn.to_string())
            .is_some()
        {
            return;
        }
        self.idempotency_order.push_back(key);
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_TOKENS {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency_tokens.remove(&oldest);
            }
        }
    }

    /// Forget every token that created `arn` or anything UNDER it (a sub-resource,
    /// recorded as `<arn>/<kind>/<id>`), so deleting a resource releases what
    /// creating it and its children allocated.
    pub fn forget_idempotency_tokens_for(&mut self, arn: &str) {
        let sub_resource_prefix = format!("{arn}/");
        self.idempotency_tokens
            .retain(|_, created| created != arn && !created.starts_with(&sub_resource_prefix));
        self.idempotency_order
            .retain(|key| self.idempotency_tokens.contains_key(key));
    }

    /// How many idempotency tokens are remembered. For tests and diagnostics.
    pub fn remembered_token_count(&self) -> usize {
        self.idempotency_tokens.len()
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsResiliencehubAwsresiliencehubState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsResiliencehubAwsresiliencehubState>)
}

/// An Application Component as the operations whose OWN output member is the
/// component report it.
///
/// `AppComponent$additionalInfo` is the component's own attribute map, and the
/// whole component record is what such an operation answers with, so a component
/// carrying no attributes reports an EMPTY map rather than no member: a client
/// reading the component back always has a map to merge into. The embedded
/// component SUMMARIES a `PhysicalResource$appComponents` carries are the stored
/// records themselves, not this projection.
pub fn reported_app_component(component: &AppComponent) -> AppComponent {
    AppComponent {
        name: component.name.clone(),
        r#type: component.r#type.clone(),
        id: component.id.clone(),
        additional_info: Some(component.additional_info.clone().unwrap_or_default()),
    }
}

/// The `tags` a resource is persisted with: the tag map the request carried, or an
/// EMPTY map when it carried none.
///
/// `tags` is how a client reads a resource's tag state, and a resource this service
/// returns always answers it: an untagged resource carries an empty map rather than
/// dropping the member, so a client can read the map unconditionally and diff it
/// against a later read without seeing a type change. Normalising on the WRITE is
/// what makes every later read agree, because a read of an application or a
/// resiliency policy is a clone of the stored record.
pub fn reported_tags(tags: Option<HashMap<String, String>>) -> Option<HashMap<String, String>> {
    Some(tags.unwrap_or_default())
}

/// The `DataLocationConstraint` a resiliency policy that names none is stored with.
///
/// The member reports the geographical constraint on where the policy's data may be
/// held, and the service resolves it on every policy rather than leaving it
/// unanswered; a caller that states no constraint has asked for none, which is what
/// this value says (one of `DATA_LOCATION_CONSTRAINT_VALUES`, the enum the model
/// gives `ResiliencyPolicy$dataLocationConstraint`).
pub const DEFAULT_DATA_LOCATION_CONSTRAINT: &str = "AnyLocation";

/// The disruption scopes a resiliency policy can state an objective for, ordered by
/// how costly an architecture recovering from that scope needs: recovering a
/// software fault needs no second copy of anything, a hardware fault a second host,
/// an Availability Zone a second zone, and a Region a second region.
///
/// Transcribed in the order the model DECLARES the values in (`smithy_query
/// query="shape" shape="DisruptionType"`: Software, Hardware, AZ, Region), which is
/// that same ascending order. The generated `DISRUPTION_TYPE_VALUES` is sorted
/// alphabetically instead, so it is not this ladder.
const DISRUPTION_SCOPE_LADDER: [&str; 4] = ["Software", "Hardware", "AZ", "Region"];

/// The `estimatedCostTier` of a resiliency policy — the member the service computes
/// itself from the policy's content, so it is answered on every policy.
///
/// What a policy costs to meet is set by the BROADEST disruption it demands recovery
/// from rather than by how tight the per-type objectives are, so the tier is the
/// cost tier sitting at the position of the policy's broadest scope in
/// [`DISRUPTION_SCOPE_LADDER`]: both ladders have four rungs, `EstimatedCostTier`'s
/// (`ESTIMATED_COST_TIER_VALUES`, L1 to L4) ascending in cost just as the scope
/// ladder ascends in the redundancy it requires.
///
/// Derived from what is stored, never remembered from a request, so a policy whose
/// disruption set changes reports the tier its current content earns.
pub fn estimated_cost_tier(policy: &HashMap<String, FailurePolicy>) -> String {
    let broadest = policy
        .keys()
        .filter_map(|disruption| {
            DISRUPTION_SCOPE_LADDER
                .iter()
                .position(|scope| scope == disruption)
        })
        .max()
        .unwrap_or(0);
    ESTIMATED_COST_TIER_VALUES[broadest.min(ESTIMATED_COST_TIER_VALUES.len() - 1)].to_string()
}

/// The key [`AppVersionContents::resources`] stores a physical resource under: its
/// whole `logicalResourceId`, which is what addresses one resource within one app
/// version.
///
/// EVERY member of the logical id takes part, each length-prefixed, so two logical
/// ids that differ only in where text falls across members (`identifier` `a/b` with
/// no stack vs `identifier` `a` in stack `b`) cannot collide on one key. A member the
/// caller left out is a distinct `-`, so an absent member and an empty one are not
/// the same key either.
pub fn logical_resource_key(logical: &LogicalResourceId) -> String {
    [
        Some(logical.identifier.as_str()),
        logical.logical_stack_name.as_deref(),
        logical.resource_group_name.as_deref(),
        logical.terraform_source_name.as_deref(),
        logical.eks_source_name.as_deref(),
    ]
    .iter()
    .map(|member| match member {
        Some(member) => format!("{}:{member}", member.len()),
        None => "-".to_string(),
    })
    .collect::<Vec<_>>()
    .join("/")
}

/// The REGION and ACCOUNT an ARN names, each `None` when the ARN carries none
/// (a global service's ARN leaves the region empty), when `arn` is not an ARN at
/// all, or when the segment would not satisfy the model's own `AwsRegion` /
/// `CustomerId` pattern — the patterns the two members this feeds
/// (`PhysicalResourceId$awsRegion`, `$awsAccountId`) are constrained by.
pub fn arn_region_and_account(arn: &str) -> (Option<&str>, Option<&str>) {
    let mut segments = arn.split(':');
    if segments.next() != Some("arn") {
        return (None, None);
    }
    // partition, then service.
    let (_, _) = (segments.next(), segments.next());
    let region = segments
        .next()
        .filter(|region| region_matches_modeled_arn(region));
    let account = segments.next().filter(|account| {
        // `CustomerId`'s modelled pattern, `^[0-9]{12}$`.
        account.len() == 12 && account.bytes().all(|byte| byte.is_ascii_digit())
    });
    (region, account)
}

/// The idempotency-map key for `operation`'s `client_token`.
fn idempotency_key(operation: &str, client_token: &str) -> String {
    format!("{operation}/{client_token}")
}

/// Whether `region` satisfies the REGION component of the `Arn` shape's `@pattern`,
/// transcribed from the model (`smithy_query query="shape" shape="Arn"` on
/// `smithy-model/com-amazonaws-resiliencehub-awsresiliencehub.json`, whose region
/// group is `([a-z]{2}-((iso[a-z]{0,1}-)|(gov-)){0,1}[a-z]+-[0-9])`): two lowercase
/// letters, a hyphen, an OPTIONAL `iso`/`iso<letter>`/`gov` infix, one or more
/// lowercase letters, a hyphen and one digit.
///
/// Every ARN this service hands back must satisfy the model's pattern, because it is
/// the same pattern the boundary validator applies when that ARN comes back in as
/// `appArn` or `policyArn` — and the region is the only part of a minted ARN the
/// caller influences.
///
/// The optional group is tried BOTH ways, exactly as the pattern's alternation does:
/// a region whose third segment merely starts with `iso` (`us-isolated-1`) matches
/// through the empty branch, so committing to the `iso` branch would reject a region
/// the pattern admits.
pub fn region_matches_modeled_arn(region: &str) -> bool {
    let Some((prefix, rest)) = region.split_once('-') else {
        return false;
    };
    if prefix.len() != 2 || !prefix.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return false;
    }

    // `((iso[a-z]{0,1}-)|(gov-)){0,1}[a-z]+-[0-9]`: the empty branch, the `gov-`
    // branch and the `iso`/`iso<letter>-` branch.
    matches_region_tail(rest)
        || rest.strip_prefix("gov-").is_some_and(matches_region_tail)
        || iso_infix_tail(rest).is_some_and(matches_region_tail)
}

/// Whether `value` satisfies the `EntityName` shape's `@pattern`, transcribed from
/// the model (`smithy_query query="operation"
/// operation="CreateAppVersionResource"`, whose `resourceName` targets
/// `EntityName` with `^[A-Za-z0-9][A-Za-z0-9_\-]{1,59}$`): an alphanumeric head,
/// then 1..59 more characters out of alphanumerics, `_` and `-`.
///
/// `PhysicalResource$resourceName` targets that shape, so a name this answers
/// `false` for is one the service cannot carry on a resource.
pub fn matches_modeled_entity_name(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(head) = characters.next() else {
        return false;
    };
    if !head.is_ascii_alphanumeric() {
        return false;
    }
    let mut tail_length = 0usize;
    for character in characters {
        if !(character.is_ascii_alphanumeric() || character == '_' || character == '-') {
            return false;
        }
        tail_length += 1;
        if tail_length > 59 {
            return false;
        }
    }
    tail_length >= 1
}

/// Whether `tail` is the pattern's `[a-z]+-[0-9]` ending.
fn matches_region_tail(tail: &str) -> bool {
    let Some((letters, digit)) = tail.split_once('-') else {
        return false;
    };
    !letters.is_empty()
        && letters.bytes().all(|byte| byte.is_ascii_lowercase())
        && digit.len() == 1
        && digit.bytes().all(|byte| byte.is_ascii_digit())
}

/// What follows the pattern's `iso[a-z]{0,1}-` infix, when `rest` opens with it.
fn iso_infix_tail(rest: &str) -> Option<&str> {
    let after_iso = rest.strip_prefix("iso")?;
    if let Some(tail) = after_iso.strip_prefix('-') {
        return Some(tail);
    }
    let mut characters = after_iso.chars();
    let letter = characters.next()?;
    if !letter.is_ascii_lowercase() {
        return None;
    }
    characters.as_str().strip_prefix('-')
}

/// The region to mint this request's ARNs in: the region of the caller's SigV4
/// credential scope, or [`DEFAULT_REGION`] when the request is unsigned or scoped
/// to a region that would make the minted ARN fail the model's own `Arn` pattern.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| region_matches_modeled_arn(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The next server-generated resource id, as the canonical 8-4-4-4-12 form AWS gives
/// a Resilience Hub application, resiliency policy or resource resolution.
///
/// OPAQUE: a fresh version-4 UUID, drawn from the operating system's random source
/// by the maintained `uuid` crate, so every nibble a client may inspect — the
/// version nibble and the variant nibble included — is the one a canonically shaped
/// identifier carries, and no part of the value encodes the minting instant, a
/// sequence number or anything else a client could read an ordering or a neighbour's
/// id out of. Two ids minted back to back are therefore unrelated, and ids of
/// different applications and policies share no derivable structure.
///
/// Unique within this emulator: 122 random bits, so the identifiers this service
/// hands out do not collide, within a run or across a restart. Server-generated,
/// never derived from the request.
fn mint_resource_id() -> String {
    ::uuid::Uuid::new_v4().hyphenated().to_string()
}

/// The refusal message for an `additionalInfo` map whose `member` holds a value
/// list outside `AdditionalInfoValueList`'s modelled `@length` — `None` when every
/// entry is within it.
///
/// A collection's own `@length` is checked against the COLLECTION MEMBER's schema,
/// and a `map<string, list<string>>` member such as `AdditionalInfoMap` has no
/// separate schema for the inner list (the framework's own
/// `outer_element_count_bound_is_not_applied_to_a_nested_map` pins that), so the
/// boundary validator cannot reach this bound and a handler taking an
/// `AdditionalInfoMap` is where it is enforced.
///
/// The sentence is NOT written here: it is built from the framework's own
/// [`::smithy_server_runtime::Violation`] and rendered by
/// `aws_smithy_schema_server::constraints::envelope`, exactly as for
/// [`enum_violation`], so a handler refusal and a boundary refusal read
/// identically — including the `<n> items` form the framework displays a
/// collection's length with.
pub fn additional_info_violation(
    member: &str,
    additional_info: &HashMap<String, Vec<String>>,
) -> Option<String> {
    // Sorted, because a `HashMap` iterates in an arbitrary order and the same
    // request must be refused with the same sentence every time.
    let mut entries: Vec<(&String, &Vec<String>)> = additional_info.iter().collect();
    entries.sort_by(|left, right| left.0.cmp(right.0));

    let violations: Vec<::smithy_server_runtime::Violation> = entries
        .into_iter()
        .filter_map(|(key, values)| {
            let kind = if values.len() < ADDITIONAL_INFO_VALUES_MIN {
                ::smithy_server_runtime::ViolationKind::MinLength(ADDITIONAL_INFO_VALUES_MIN as i64)
            } else if values.len() > ADDITIONAL_INFO_VALUES_MAX {
                ::smithy_server_runtime::ViolationKind::MaxLength(ADDITIONAL_INFO_VALUES_MAX as i64)
            } else {
                return None;
            };
            Some(::smithy_server_runtime::Violation {
                // A map ENTRY's path embeds the caller's key, so it is elided for
                // the same reason the value is.
                path: elided(&format!(
                    "{member}.{}",
                    ::aws_smithy_schema_server::constraints::path::entry(key)
                )),
                kind,
                value: Some(format!("{} items", values.len())),
            })
        })
        .collect();

    ::aws_smithy_schema_server::constraints::envelope(&violations)
}

/// Mint a fresh `appArn`: `arn:aws:resiliencehub:<region>:<account>:app/<id>`.
pub fn mint_app_arn(region: &str) -> String {
    ::engine::emulator_arn(
        "resiliencehub",
        region,
        &format!("app/{}", mint_resource_id()),
    )
}

/// Mint a fresh `policyArn`:
/// `arn:aws:resiliencehub:<region>:<account>:resiliency-policy/<id>`.
pub fn mint_policy_arn(region: &str) -> String {
    ::engine::emulator_arn(
        "resiliencehub",
        region,
        &format!("resiliency-policy/{}", mint_resource_id()),
    )
}

/// The refusal message for `value`, when the model's enum for `member` does not
/// list it — `None` when it does.
///
/// The boundary constraint validator already walks every enum MEMBER of an input
/// shape, so a handler never has to re-check one. What it does not walk is a
/// map's KEY: `DisruptionPolicy`'s key targets the `DisruptionType` enum, and an
/// unknown key reaches the handler. The sentence is NOT written here — it is built
/// from the framework's own [`::smithy_server_runtime::Violation`] and rendered by
/// `aws_smithy_schema_server::constraints::envelope`, the one place that wording
/// lives, so a handler refusal and a boundary refusal read identically.
pub fn enum_violation(member: &str, value: &str, allowed: &[&str]) -> Option<String> {
    if allowed.contains(&value) {
        return None;
    }
    let violation = ::smithy_server_runtime::Violation {
        // Elided like the value: for a map KEY the path embeds caller-supplied
        // text, so it is the same amplification vector.
        path: elided(member),
        kind: ::smithy_server_runtime::ViolationKind::EnumValue(
            allowed.iter().map(|allowed| (*allowed).to_string()).collect(),
        ),
        // `Violation::value` must arrive ALREADY elided: the message interpolates
        // it verbatim, and this one is a caller-supplied string of any length.
        value: Some(elided(value)),
    };
    ::aws_smithy_schema_server::constraints::envelope(std::slice::from_ref(&violation))
}

/// The longest caller-supplied text a refusal message echoes, pinned to the
/// boundary validator's own ceiling (`MAX_ECHOED_VALUE` in
/// `aws_smithy_schema_server::constraints`).
const MAX_ECHOED_VALUE: usize = 256;

/// `text` as a refusal message may carry it: itself when short, a
/// character-boundary-safe prefix with an elision when not, so a huge request
/// member cannot be amplified into a huge response.
///
/// The framework's equivalent (`constraints::echoed`) is PRIVATE and lives under
/// the agent-immutable `vendor/` tree, so it cannot be called or widened from
/// here; the ceiling is therefore pinned to the same constant above.
fn elided(text: &str) -> String {
    if text.len() <= MAX_ECHOED_VALUE {
        return text.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &text[..end], text.len())
}

/// "Now", as the wire timestamp type every creation/evaluation time uses.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now())
}
