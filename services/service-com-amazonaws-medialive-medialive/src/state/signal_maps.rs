// SPDX-License-Identifier: CC0-1.0

//! The SIGNAL MAP slice: the signal map record, its monitor deployments, the
//! media-resource map a discovery walks out of the channel and input stores,
//! and the refusals a signal map's delete and update answer with.
//!
//! One file for the whole slice, so the operations that read and write a signal
//! map cannot drift into several readings of one rule. Moved here unchanged
//! from the state root, which re-exports it (`pub use signal_maps::*;`), so
//! every `crate::state::…` path a handler already uses still resolves.

use super::*;

/// Why a SIGNAL MAP's delete removed nothing. Each variant is one modelled
/// refusal, and each of them leaves the store exactly as it was.
///
/// The two conflicts are the one class the delete of a resource with
/// asynchronous lifecycle actions refuses on: work on that resource is still in
/// flight, so there is nothing to delete it out from under yet. Each carries the
/// id of the signal map and the status it is in, so the handler composes its
/// message out of STATE rather than out of the request.
#[derive(Debug)]
pub enum SignalMapDeleteRefusal {
    /// No signal map answers to the identifier, by either form `Identifier`
    /// documents (its id or its current name).
    NotFound,
    /// A lifecycle action of the signal map itself is still running, so the
    /// signal map is not in a state a delete may take it out of.
    LifecycleRunning { id: String, status: String },
    /// A monitor deployment of the signal map is still running — the same
    /// in-flight deployment `StartMonitorDeployment` and
    /// `StartDeleteMonitorDeployment` refuse a second one over.
    DeploymentRunning { id: String, status: String },
}

/// Why [`ComAmazonawsMedialiveMedialiveState::start_signal_map_update`] changed
/// nothing. Each variant is one modelled refusal of `StartUpdateSignalMap`, and
/// each of them leaves the store exactly as it was.
#[derive(Debug)]
pub enum SignalMapUpdateRefusal {
    /// No signal map answers to the identifier, by either form `Identifier`
    /// documents (its id or its current name).
    NotFound,
    /// A lifecycle action of the signal map itself is still running, so the
    /// signal map is not in a state a further update may take it out of.
    /// Carries the id and the status it is in.
    LifecycleRunning { id: String, status: String },
    /// A monitor deployment of the signal map is still running — the same
    /// in-flight deployment `DeleteSignalMap`, `StartMonitorDeployment` and
    /// `StartDeleteMonitorDeployment` refuse over. Carries the id and the
    /// deployment status it is in.
    DeploymentRunning { id: String, status: String },
    /// Another signal map already carries the name asked for, which `Name` is
    /// documented to make unique within the resource type in a region.
    NameTaken,
    /// The `DiscoveryEntryPointArn` the request supplied is not an AWS resource
    /// ARN at all, so it names no resource to discover a graph from. Carries the
    /// value the request supplied.
    EntryPointMalformed { arn: String },
    /// The entry point a rediscovery would walk from names no resource this
    /// store holds, so there is no workflow to map. Carries the ARN that
    /// resolved to nothing.
    EntryPointNotFound { arn: String },
    /// A `CloudWatchAlarmTemplateGroupIdentifiers` entry names no stored alarm
    /// template group, so there is no group to associate the signal map with.
    /// Carries the identifier that resolved to nothing.
    AlarmGroupNotFound { identifier: String },
    /// An `EventBridgeRuleTemplateGroupIdentifiers` entry names no stored rule
    /// template group, on the same terms as [`Self::AlarmGroupNotFound`].
    RuleGroupNotFound,
}

/// What one `StartUpdateSignalMap` request asks the store to change: a member
/// the request carried is `Some` and replaces the stored one, a member it
/// omitted is `None` and leaves that setting as it was.
#[derive(Debug, Clone, Default)]
pub struct SignalMapChanges {
    pub name: Option<String>,
    pub description: Option<String>,
    /// The entry point to discover the signal map's graph from. A CHANGED one
    /// makes the update rediscover the graph, which is what
    /// `StartUpdateSignalMap` documents ("Will discover a new signal map if a
    /// changed discoveryEntryPointArn is provided").
    pub discovery_entry_point_arn: Option<String>,
    /// `ForceRediscovery`: "If true, will force a rediscovery of a signal map if
    /// an unchanged discoveryEntryPointArn is provided."
    pub force_rediscovery: bool,
    /// The identifiers of the alarm template groups to associate the signal map
    /// with, each resolved to a group id before anything is written.
    pub cloud_watch_alarm_template_group_identifiers: Option<Vec<String>>,
    /// The identifiers of the eventbridge rule template groups to associate the
    /// signal map with, resolved on the same terms.
    pub event_bridge_rule_template_group_identifiers: Option<Vec<String>>,
}

impl ComAmazonawsMedialiveMedialiveState {
    /// Whether the signal-map store is full, so `CreateSignalMap` must refuse
    /// rather than mint another signal map.
    pub fn signal_maps_full(&self) -> bool {
        self.signal_maps.len() >= MAX_SIGNAL_MAPS
    }

    /// The next signal map id: a numeric string of the length the model's
    /// `__stringMin7Max11PatternAws097` allows.
    pub fn mint_signal_map_id(&mut self) -> String {
        (FIRST_SIGNAL_MAP_ID + self.mints.next(|mints| &mut mints.signal_map)).to_string()
    }

    /// Record `token` as having created `signal_map_id`.
    pub fn record_signal_map_token(&mut self, token: String, signal_map_id: &str) {
        self.signal_map_tokens
            .record(token, signal_map_id, MAX_SIGNAL_MAPS);
    }

    /// The signal map a stored `RequestId` created, if it still exists.
    pub fn signal_map_for_token(&self, request_id: &str) -> Option<&str> {
        let id = self.signal_map_tokens.created(request_id)?;
        self.signal_maps.get(id).map(|map| map.id.as_str())
    }

    /// The id of the signal map `identifier` names: `GetSignalMap`'s
    /// `Identifier` is documented as "A signal map's identifier. Can be either
    /// be its id or current name", so an id is tried first and then a name.
    pub fn signal_map_id_for_identifier(&self, identifier: &str) -> Option<&str> {
        if let Some(map) = self.signal_maps.get(identifier) {
            return Some(map.id.as_str());
        }
        self.signal_map_named(identifier)
    }

    /// The id of the signal map called `name`, if one exists. A signal map's
    /// `Name` is documented as unique "within the scope of a resource type in a
    /// specific region", which is what makes this lookup single-valued.
    pub fn signal_map_named(&self, name: &str) -> Option<&str> {
        self.signal_maps
            .values()
            .find(|map| map.name == name)
            .map(|map| map.id.as_str())
    }

    /// The signal map `identifier` names, for a MUTATION of it — resolved by id
    /// or by current name, exactly as [`Self::signal_map_id_for_identifier`]
    /// resolves it for a read, so the two cannot disagree about which signal map
    /// an identifier names.
    pub fn signal_map_for_identifier_mut(
        &mut self,
        identifier: &str,
    ) -> Option<&mut SignalMapRecord> {
        let id = self.signal_map_id_for_identifier(identifier)?.to_string();
        self.signal_maps.get_mut(&id)
    }

    /// Remove the signal map `identifier` names, answering the record that was
    /// removed — or refuse.
    ///
    /// `DeleteSignalMap` "deletes the specified signal map" and documents no
    /// precondition, so the only refusals are an identifier that resolves to no
    /// signal map and a signal map with work of its own still in flight: its own
    /// lifecycle action, or a monitor deployment of it. Once removed the signal
    /// map is GONE from the store — `SignalMapStatus` declares no deleted state
    /// for a reader to go on reporting it in — so every later read of either of
    /// its identifiers misses, its name is free again, its tags go with it and
    /// the template groups it named are no longer held by it.
    ///
    /// The resolution, the preconditions and the removal live together here, so a
    /// handler cannot check one of them and write past another; the handler's job
    /// is only to turn a [`SignalMapDeleteRefusal`] into the operation's own
    /// modelled error.
    pub fn delete_signal_map(
        &mut self,
        identifier: &str,
    ) -> Result<SignalMapRecord, SignalMapDeleteRefusal> {
        let id = self
            .signal_map_id_for_identifier(identifier)
            .ok_or(SignalMapDeleteRefusal::NotFound)?
            .to_string();
        let map = self
            .signal_maps
            .get(&id)
            .ok_or(SignalMapDeleteRefusal::NotFound)?;

        if map.lifecycle_in_progress() {
            return Err(SignalMapDeleteRefusal::LifecycleRunning {
                id,
                status: map.status().to_string(),
            });
        }
        if map.monitor_deployment_in_progress() {
            return Err(SignalMapDeleteRefusal::DeploymentRunning {
                id,
                status: map.monitor_deployment_status().to_string(),
            });
        }

        let removed = self
            .signal_maps
            .remove(&id)
            .ok_or(SignalMapDeleteRefusal::NotFound)?;
        // A token never outlives the resource it minted, so a retried create
        // cannot resurrect a deleted signal map.
        self.signal_map_tokens.forget(&id);
        Ok(removed)
    }

    /// Apply `changes` to the signal map `identifier` names and put it into the
    /// `UPDATE_IN_PROGRESS` status the clock settles, answering its id — or
    /// refuse.
    ///
    /// `StartUpdateSignalMap` "initiates an update for the specified signal map.
    /// Will discover a new signal map if a changed discoveryEntryPointArn is
    /// provided", and `ForceRediscovery` forces that rediscovery for an
    /// unchanged one — so a rediscovery walks the store exactly as the create's
    /// discovery does, and the graph it finds replaces the stored one when the
    /// update completes. Every other member is a partial update: one the request
    /// carried replaces what is stored, one it omitted leaves it alone.
    ///
    /// Like every other mutation of a signal map, an update needs the signal map
    /// to have no asynchronous work of its own in flight: neither its own
    /// lifecycle action nor a monitor deployment of it, which deploys the very
    /// monitor contents an update rewrites.
    ///
    /// The resolution, the preconditions, the rediscovery and the write live
    /// together here, so a handler cannot check one of them and write past
    /// another; the handler's job is only to turn a [`SignalMapUpdateRefusal`]
    /// into the operation's own modelled error.
    pub fn start_signal_map_update(
        &mut self,
        region: &str,
        identifier: &str,
        changes: SignalMapChanges,
        now: SystemTime,
    ) -> Result<String, SignalMapUpdateRefusal> {
        let id = self
            .signal_map_id_for_identifier(identifier)
            .ok_or(SignalMapUpdateRefusal::NotFound)?
            .to_string();
        let map = self
            .signal_maps
            .get(&id)
            .ok_or(SignalMapUpdateRefusal::NotFound)?;

        // A lifecycle action of the signal map is asynchronous, so one still in
        // flight is already changing the resource this call would change.
        if map.lifecycle_in_progress() {
            return Err(SignalMapUpdateRefusal::LifecycleRunning {
                id,
                status: map.status().to_string(),
            });
        }
        // A monitor deployment is asynchronous too, and it deploys the monitor
        // this update would replace — the graph the signal map maps and the
        // template groups it is associated with — so one still in flight is
        // already working on what this call would change.
        if map.monitor_deployment_in_progress() {
            return Err(SignalMapUpdateRefusal::DeploymentRunning {
                id,
                status: map.monitor_deployment_status().to_string(),
            });
        }
        let stored_entry_point = map.discovery_entry_point_arn.clone();

        // `Name` is unique "within the scope of a resource type in a specific
        // region", so a rename cannot take a name another signal map holds —
        // while renaming a signal map to the name it already has changes
        // nothing and is not a collision with itself.
        if let Some(name) = changes.name.as_deref() {
            if self.signal_map_named(name).is_some_and(|held| held != id) {
                return Err(SignalMapUpdateRefusal::NameTaken);
            }
        }

        // Request SYNTAX first: a `DiscoveryEntryPointArn` that is not an ARN at
        // all cannot name a resource, whichever operation supplied it.
        let requested_entry_point = changes.discovery_entry_point_arn.as_deref();
        if let Some(arn) = requested_entry_point {
            if !is_resource_arn(arn) {
                return Err(SignalMapUpdateRefusal::EntryPointMalformed {
                    arn: arn.to_string(),
                });
            }
        }

        // Then rediscovery: a changed entry point discovers a new graph, and an
        // unchanged one only when the request forces it. The graph is resolved
        // against the resources that exist now, so an entry point naming none is
        // refused and nothing is written.
        let entry_point = requested_entry_point
            .unwrap_or(stored_entry_point.as_str())
            .to_string();
        let rediscover = requested_entry_point.is_some_and(|arn| arn != stored_entry_point)
            || changes.force_rediscovery;
        let discovered = if rediscover {
            Some(self.discover_media_resource_map(&entry_point).ok_or(
                SignalMapUpdateRefusal::EntryPointNotFound { arn: entry_point },
            )?)
        } else {
            None
        };

        // A group identifier names a group that must already exist, and the
        // signal map records the ID of the group it resolved — never the
        // identifier it was given — exactly as the create resolves them.
        let mut alarm_group_ids = None;
        if let Some(identifiers) = changes.cloud_watch_alarm_template_group_identifiers.as_ref() {
            let mut resolved = Vec::with_capacity(identifiers.len());
            for identifier in identifiers {
                let group = self.alarm_template_group(region, identifier).ok_or_else(|| {
                    SignalMapUpdateRefusal::AlarmGroupNotFound {
                        identifier: identifier.clone(),
                    }
                })?;
                resolved.push(group.id.clone());
            }
            alarm_group_ids = Some(resolved);
        }
        let mut rule_group_ids = None;
        if let Some(identifiers) = changes
            .event_bridge_rule_template_group_identifiers
            .as_ref()
        {
            let mut resolved = Vec::with_capacity(identifiers.len());
            for identifier in identifiers {
                let group = self
                    .event_bridge_rule_template_group(region, identifier)
                    .ok_or(SignalMapUpdateRefusal::RuleGroupNotFound)?;
                resolved.push(group.id.clone());
            }
            rule_group_ids = Some(resolved);
        }

        // Every precondition is met, so the change is written in one step.
        let map = self
            .signal_maps
            .get_mut(&id)
            .ok_or(SignalMapUpdateRefusal::NotFound)?;
        if let Some(name) = changes.name {
            map.name = name;
        }
        if let Some(description) = changes.description {
            map.description = Some(description);
        }
        if let Some(arn) = changes.discovery_entry_point_arn {
            map.discovery_entry_point_arn = arn;
        }
        if let Some(ids) = alarm_group_ids {
            map.cloud_watch_alarm_template_group_ids = ids;
        }
        if let Some(ids) = rule_group_ids {
            map.event_bridge_rule_template_group_ids = ids;
        }
        map.begin_update(discovered, now);
        Ok(id)
    }

    /// The input whose ARN is exactly `arn`, if this store holds one — the same
    /// global-identity reading of an ARN as [`Self::channel_by_arn`].
    pub(super) fn input_by_arn(&self, arn: &str) -> Option<&InputRecord> {
        self.inputs.values().find(|input| input.arn == arn)
    }

    /// Both directions of every channel-to-input attachment this store holds.
    ///
    /// Built once per discovery and then read by every node the walk visits, so
    /// a discovery costs one pass over the store's attachments rather than a
    /// re-derivation per node; the de-duplication of a channel's
    /// `InputAttachments` goes through a set, so one channel's pass is linear in
    /// the attachments it carries ([`MAX_INPUT_ATTACHMENTS`] of them at most).
    pub(super) fn attachment_edges(&self) -> AttachmentEdges<'_> {
        let mut edges = AttachmentEdges::default();
        for channel in self.channels.values() {
            let mut seen: BTreeSet<&str> = BTreeSet::new();
            for id in channel
                .input_attachments
                .iter()
                .filter_map(|attachment| attachment.input_id.as_deref())
            {
                if !seen.insert(id) {
                    continue;
                }
                if let Some(input) = self.inputs.get(id).filter(|input| !input.is_deleted()) {
                    edges
                        .inputs_of
                        .entry(channel.arn.as_str())
                        .or_default()
                        .push(input);
                    edges
                        .channels_of
                        .entry(input.arn.as_str())
                        .or_default()
                        .push(channel);
                }
            }
        }
        edges
    }

    /// One discovered channel as a graph node: upstream (`Sources`) the channel
    /// it follows and the inputs attached to it, downstream (`Destinations`) the
    /// channels that follow it.
    pub(super) fn channel_media_resource(
        &self,
        channel: &ChannelRecord,
        edges: &AttachmentEdges<'_>,
    ) -> MediaResource {
        let mut sources: Vec<MediaResourceNeighbor> = channel
            .linked_channel_settings
            .as_ref()
            .and_then(|linked| linked.follower_channel_settings.as_ref())
            .and_then(|follower| follower.primary_channel_arn.as_deref())
            .and_then(|primary| self.channel_by_arn(primary))
            .map(|primary| vec![neighbor(&primary.arn, primary.name.as_deref())])
            .unwrap_or_default();
        // An attached input FEEDS the channel, so it is one of the channel's
        // sources, read out of the channel's stored `InputAttachments`.
        sources.extend(
            edges
                .inputs_of
                .get(channel.arn.as_str())
                .into_iter()
                .flatten()
                .map(|input| neighbor(&input.arn, input.name.as_deref())),
        );
        MediaResource {
            name: channel.name.clone(),
            sources: Some(sources),
            destinations: Some(
                self.following_channel_arns(&channel.arn)
                    .iter()
                    .filter_map(|follower| self.channel_by_arn(follower))
                    .map(|follower| neighbor(&follower.arn, follower.name.as_deref()))
                    .collect(),
            ),
        }
    }

    /// One discovered input as a graph node: downstream (`Destinations`) the
    /// channels whose `InputAttachments` name it, which is the same edge the
    /// channel reports as a source, read from the other end.
    ///
    /// `Sources` is empty: what feeds an input is a pull URL, a push endpoint,
    /// a MediaConnect flow or a device — none of them a resource this store
    /// holds, so none of them is claimed as a neighbour.
    pub(super) fn input_media_resource(
        &self,
        input: &InputRecord,
        edges: &AttachmentEdges<'_>,
    ) -> MediaResource {
        MediaResource {
            name: input.name.clone(),
            sources: Some(Vec::new()),
            destinations: Some(
                edges
                    .channels_of
                    .get(input.arn.as_str())
                    .into_iter()
                    .flatten()
                    .map(|channel| neighbor(&channel.arn, channel.name.as_deref()))
                    .collect(),
            ),
        }
    }

    /// The graph node the resource `arn` names, if this store holds one: a
    /// channel, or an input that has not been deleted.
    pub(super) fn media_resource_at(&self, arn: &str, edges: &AttachmentEdges<'_>) -> Option<MediaResource> {
        if let Some(channel) = self.channel_by_arn(arn) {
            return Some(self.channel_media_resource(channel, edges));
        }
        self.input_by_arn(arn)
            .filter(|input| !input.is_deleted())
            .map(|input| self.input_media_resource(input, edges))
    }

    /// Discover the media workflow graph reachable from the resource
    /// `entry_point` names, keyed by each discovered resource's ARN.
    ///
    /// The ENTRY POINT is a channel: `DiscoveryEntryPointArn` is "A top-level
    /// supported AWS resource ARN to discovery a signal map from", and a channel
    /// is the top-level resource this store holds; an ARN naming anything else
    /// (another resource type of this service, another service, another account
    /// or region) resolves to no entry point and answers `None` — which the
    /// caller renders as the operation's modelled `NotFoundException`.
    ///
    /// The NODES are every media resource the walk reaches from there: channels
    /// and the inputs those channels attach, each keyed by its own ARN. The walk
    /// is breadth-first over the links the discovered resources themselves
    /// declare (a follower channel's primary, the followers that name it, and
    /// the inputs a channel's `InputAttachments` name), so the graph is read out
    /// of persisted state. It is the SNAPSHOT of that state at the instant
    /// discovery runs: the real
    /// service rediscovers a signal map's graph only when a discovery is asked
    /// for (`StartUpdateSignalMap`, with a changed entry point or
    /// `ForceRediscovery`), so a resource created or deleted afterwards does not
    /// change a stored signal map's `MediaResourceMap`.
    pub fn discover_media_resource_map(
        &self,
        entry_point: &str,
    ) -> Option<BTreeMap<String, MediaResource>> {
        self.channel_by_arn(entry_point)?;

        // One pass over the store's attachments, read by every node below.
        let edges = self.attachment_edges();
        let mut discovered: BTreeMap<String, MediaResource> = BTreeMap::new();
        let mut pending: VecDeque<String> = VecDeque::new();
        pending.push_back(entry_point.to_string());

        while let Some(arn) = pending.pop_front() {
            if discovered.contains_key(&arn) {
                continue;
            }
            let Some(resource) = self.media_resource_at(&arn, &edges) else {
                continue;
            };
            for next in resource
                .sources
                .iter()
                .chain(resource.destinations.iter())
                .flatten()
            {
                pending.push_back(next.arn.clone());
            }
            discovered.insert(arn, resource);
        }
        Some(discovered)
    }

    /// The full description of one stored signal map, or `None` when no signal
    /// map has that id.
    pub fn signal_map_description(&self, id: &str) -> Option<SignalMapDescription> {
        let map = self.signal_maps.get(id)?;
        // A discovered graph is reported once a discovery of this signal map has
        // COMPLETED — which is also what stamps `LastDiscoveredAt` — so a later
        // lifecycle action (an update) does not blank out the graph the signal
        // map still maps while that action runs.
        let discovered = map.last_discovered_at.is_some();
        Some(SignalMapDescription {
            arn: Some(map.arn.clone()),
            // The ids of the cloudwatch alarm template groups the create
            // resolved, or ABSENT when the request named none — absent rather
            // than empty, because `CloudWatchAlarmTemplateGroupIds` reports the
            // groups the signal map is associated with and an association the
            // request never asked for is not an empty list of associations.
            cloud_watch_alarm_template_group_ids: (!map
                .cloud_watch_alarm_template_group_ids
                .is_empty())
                .then(|| map.cloud_watch_alarm_template_group_ids.clone()),
            // The ids of the eventbridge rule template groups the create
            // resolved, or ABSENT when the request named none.
            event_bridge_rule_template_group_ids: (!map
                .event_bridge_rule_template_group_ids
                .is_empty())
                .then(|| map.event_bridge_rule_template_group_ids.clone()),
            created_at: Some(timestamp(map.created_at)),
            description: map.description.clone(),
            discovery_entry_point_arn: Some(map.discovery_entry_point_arn.clone()),
            // ABSENT while nothing failed: `ErrorMessage` is "Error message
            // associated with a failed creation or failed update attempt", and a
            // stored signal map's discovery resolved its entry point.
            error_message: None,
            failed_media_resource_map: None,
            id: Some(map.id.clone()),
            // The discovered graph and the instant it was discovered are
            // reported only once the clock has made discovery due; while the
            // signal map is CREATE_IN_PROGRESS there is nothing discovered yet.
            last_discovered_at: map.last_discovered_at.map(timestamp),
            media_resource_map: discovered.then(|| map.media_resource_map.clone()),
            modified_at: Some(timestamp(map.modified_at)),
            // The monitor side of the signal map, out of the record: the latest
            // deployment `StartMonitorDeployment` started and the latest one
            // that succeeded, both ABSENT while there is none.
            //
            // `MonitorChangesPendingDeployment` is "if true, there are pending
            // monitor changes for this signal map that can be deployed": no
            // operation in this emulator changes a signal map's monitor, so
            // there is never a pending change to deploy.
            monitor_changes_pending_deployment: Some(false),
            monitor_deployment: map
                .monitor_deployment
                .as_ref()
                .map(MonitorDeploymentRecord::described),
            last_successful_monitor_deployment: map.last_successful_monitor_deployment.clone(),
            name: Some(map.name.clone()),
            status: Some(map.status.clone()),
            tags: Some(map.tags.clone()),
        })
    }

    /// The `SignalMapSummary` projection of one stored signal map, as
    /// `ListSignalMaps` reports it.
    ///
    /// Every member is read off the stored record or derived from it, exactly as
    /// [`Self::signal_map_description`] reads the same record for the flat
    /// outputs: the summary is one view of one signal map, so the two cannot
    /// disagree about its `Status`, its timestamps or its tags.
    pub fn signal_map_summary(&self, map: &SignalMapRecord) -> SignalMapSummary {
        SignalMapSummary {
            arn: map.arn.clone(),
            created_at: timestamp(map.created_at),
            description: map.description.clone(),
            id: map.id.clone(),
            modified_at: Some(timestamp(map.modified_at)),
            // The status of the signal map's latest monitor deployment, read off
            // the same record `signal_map_description` reports
            // `MonitorDeployment` from, so the summary and the full description
            // cannot disagree about it.
            monitor_deployment_status: map.monitor_deployment_status().to_string(),
            name: map.name.clone(),
            status: map.status.clone(),
            tags: Some(map.tags.clone()),
        }
    }
}

/// What a monitor deployment of a signal map DOES, which is what decides the
/// pair of statuses it moves through and what completing it leaves behind.
///
/// `StartMonitorDeployment` "initiates a deployment to deploy the latest monitor
/// of the specified signal map" — a real one, or the dry run its `DryRun` member
/// asks for — and `StartDeleteMonitorDeployment` "initiates a deployment to
/// delete the monitor of the specified signal map". All three are the same
/// asynchronous deployment of one signal map's monitor, differing only in what
/// they leave behind, so one record models them and this is the kind of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MonitorDeploymentKind {
    /// Deploy the signal map's latest monitor.
    Deploy,
    /// Run the deployment without deploying anything.
    DryRun,
    /// Delete the signal map's monitor.
    Delete,
}

impl MonitorDeploymentKind {
    /// The `SignalMapMonitorDeploymentStatus` a deployment of this kind runs in.
    pub(super) fn in_progress_status(self) -> &'static str {
        match self {
            Self::Deploy => signal_map_monitor_deployment_status::DEPLOYMENT_IN_PROGRESS,
            Self::DryRun => signal_map_monitor_deployment_status::DRY_RUN_DEPLOYMENT_IN_PROGRESS,
            Self::Delete => signal_map_monitor_deployment_status::DELETE_IN_PROGRESS,
        }
    }

    /// The status a deployment of this kind completes in.
    pub(super) fn complete_status(self) -> &'static str {
        match self {
            Self::Deploy => signal_map_monitor_deployment_status::DEPLOYMENT_COMPLETE,
            Self::DryRun => signal_map_monitor_deployment_status::DRY_RUN_DEPLOYMENT_COMPLETE,
            Self::Delete => signal_map_monitor_deployment_status::DELETE_COMPLETE,
        }
    }
}

/// What completing a monitor deployment does to the signal map's
/// `LastSuccessfulMonitorDeployment`, "the latest successful monitor deployment
/// of a signal map".
pub(super) enum LatestSuccessfulDeployment {
    /// A deployment that deployed the monitor IS that latest successful
    /// deployment.
    Deployed(SuccessfulMonitorDeployment),
    /// A dry run deploys nothing, so it neither becomes the latest successful
    /// deployment nor undoes the one before it.
    Unchanged,
    /// A completed deletion leaves the signal map with no deployed monitor, so
    /// there is no successful deployment of one left to report.
    Gone,
}

/// One monitor deployment of one signal map, as the operation that started it
/// started it.
///
/// A deployment is ASYNCHRONOUS — the operation answers 202 and the caller polls
/// the signal map's `MonitorDeployment$Status` — so the record carries the status
/// it is in now and the instant the clock makes its outcome due. The instant is
/// STORED and never slept on.
#[derive(Debug, Clone)]
pub struct MonitorDeploymentRecord {
    /// What this deployment does, which is what decides both the pair of
    /// statuses it moves through and what completing it leaves behind.
    pub(super) kind: MonitorDeploymentKind,
    /// The `SignalMapMonitorDeploymentStatus` this deployment is in.
    pub(super) status: String,
    /// `MonitorDeployment$DetailsUri`, built once when the deployment started
    /// ([`monitor_deployment_details_uri`]) so every later read reports the same
    /// URI.
    pub(super) details_uri: String,
    /// The instant this deployment's outcome becomes due.
    pub(super) due: SystemTime,
}

impl MonitorDeploymentRecord {
    /// Whether this deployment is still running, in the IN_PROGRESS status of
    /// its own kind.
    pub(super) fn in_progress(&self) -> bool {
        self.status == self.kind.in_progress_status()
    }

    /// This deployment as the modelled `MonitorDeployment`, "the latest monitor
    /// deployment of a signal map".
    pub(super) fn described(&self) -> MonitorDeployment {
        MonitorDeployment {
            details_uri: Some(self.details_uri.clone()),
            // ABSENT while nothing failed: `MonitorDeployment$ErrorMessage` is
            // the "error message associated with a FAILED monitor deployment",
            // and no deployment fails in this emulator.
            error_message: None,
            status: self.status.clone(),
        }
    }
}

/// One created signal map, as `CreateSignalMap` stored it plus the graph its
/// discovery found, the lifecycle state of that discovery and the monitor
/// deployments `StartMonitorDeployment` has run for it.
#[derive(Debug, Clone)]
pub struct SignalMapRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub discovery_entry_point_arn: String,
    /// The ids of the eventbridge rule template groups the create resolved from
    /// the identifiers the request named — ids this store holds, never the
    /// identifiers themselves.
    pub event_bridge_rule_template_group_ids: Vec<String>,
    /// The ids of the cloudwatch alarm template groups the create resolved, on
    /// the same terms as the eventbridge ones beside them.
    pub cloud_watch_alarm_template_group_ids: Vec<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    /// Updated when the signal map changes — which, for a signal map nothing
    /// has updated yet, is when its discovery completed.
    pub(super) modified_at: SystemTime,
    /// The graph discovery found, keyed by resource ARN. Reported only once
    /// discovery has come due.
    pub(super) media_resource_map: BTreeMap<String, MediaResource>,
    /// The `SignalMapStatus` this signal map is in.
    pub(super) status: String,
    /// The instant discovery becomes due, so `CREATE_IN_PROGRESS` settles into
    /// `CREATE_COMPLETE`. Stored, never slept on.
    pub(super) discovery_due: SystemTime,
    /// The instant an update started by `StartUpdateSignalMap` becomes due, so
    /// `UPDATE_IN_PROGRESS` settles into `UPDATE_COMPLETE`, or `None` while no
    /// update is running. Stored, never slept on, on the same terms as
    /// [`Self::discovery_due`].
    pub(super) update_due: Option<SystemTime>,
    /// The graph a running update's rediscovery found, held until that update
    /// comes due: until then the signal map still reports the graph its last
    /// completed discovery produced, because that is the one it currently maps.
    pub(super) pending_media_resource_map: Option<BTreeMap<String, MediaResource>>,
    pub(super) last_discovered_at: Option<SystemTime>,
    /// The latest monitor deployment `StartMonitorDeployment` started for this
    /// signal map, or `None` while none ever was — which is the `NOT_DEPLOYED`
    /// status [`Self::monitor_deployment_status`] reports.
    pub(super) monitor_deployment: Option<MonitorDeploymentRecord>,
    /// The latest monitor deployment that COMPLETED, which only a real
    /// (non-dry-run) deployment leaves behind: a dry run deploys nothing, so it
    /// is not a successful monitor deployment OF the signal map.
    pub(super) last_successful_monitor_deployment: Option<SuccessfulMonitorDeployment>,
}

impl SignalMapRecord {
    /// Materialise what the clock has already made due: the discovery, owed at
    /// `discovery_due`, and the outcome of a monitor deployment, owed at its own
    /// `due`. Both were owed at those instants whether or not anybody called, so
    /// applying them in a reporting operation changes nothing a later caller
    /// would have seen.
    pub(super) fn settle(&mut self, now: SystemTime) {
        if self.status == signal_map_status::CREATE_IN_PROGRESS && now >= self.discovery_due {
            self.status = signal_map_status::CREATE_COMPLETE.to_string();
            self.last_discovered_at = Some(self.discovery_due);
            self.modified_at = self.discovery_due;
        }
        // The update an earlier call started, owed at its own due instant: the
        // signal map reaches `UPDATE_COMPLETE` and, when that update asked for a
        // rediscovery, the graph it found becomes the graph the signal map maps,
        // discovered at the instant the update completed.
        if let Some(due) = self
            .update_due
            .filter(|due| self.status == signal_map_status::UPDATE_IN_PROGRESS && now >= *due)
        {
            self.update_due = None;
            self.status = signal_map_status::UPDATE_COMPLETE.to_string();
            if let Some(discovered) = self.pending_media_resource_map.take() {
                self.media_resource_map = discovered;
                self.last_discovered_at = Some(due);
            }
            self.modified_at = due;
        }
        // Completed on THIS call: the due instant to stamp as the signal map's
        // `ModifiedAt`, and what that completion does to the latest successful
        // deployment. Read out of the deployment before the signal map's own
        // fields are written, so the record is borrowed once.
        let settled = self.monitor_deployment.as_mut().and_then(|deployment| {
            if !deployment.in_progress() || now < deployment.due {
                return None;
            }
            deployment.status = deployment.kind.complete_status().to_string();
            let latest = match deployment.kind {
                MonitorDeploymentKind::Deploy => {
                    LatestSuccessfulDeployment::Deployed(SuccessfulMonitorDeployment {
                        details_uri: deployment.details_uri.clone(),
                        status: deployment.status.clone(),
                    })
                }
                MonitorDeploymentKind::DryRun => LatestSuccessfulDeployment::Unchanged,
                MonitorDeploymentKind::Delete => LatestSuccessfulDeployment::Gone,
            };
            Some((deployment.due, latest))
        });
        if let Some((due, latest)) = settled {
            self.modified_at = due;
            match latest {
                LatestSuccessfulDeployment::Deployed(successful) => {
                    self.last_successful_monitor_deployment = Some(successful);
                }
                LatestSuccessfulDeployment::Unchanged => {}
                LatestSuccessfulDeployment::Gone => {
                    self.last_successful_monitor_deployment = None;
                }
            }
        }
    }

    /// The `SignalMapStatus` this signal map is in.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Whether this signal map has a monitor to deploy at all: deploying "the
    /// latest monitor of the specified signal map" needs the media resource map
    /// its discovery found, so a signal map whose discovery has not completed
    /// has nothing to deploy yet — and neither has one whose own lifecycle
    /// action is still in flight, which is the graph being rebuilt underneath.
    pub fn discovery_complete(&self) -> bool {
        self.last_discovered_at.is_some() && !self.lifecycle_in_progress()
    }

    /// Whether a lifecycle action of the signal map ITSELF is still running, so
    /// nothing may take the signal map out from under it. Those are the
    /// `SignalMapStatus` values whose name ends in `_IN_PROGRESS` — the
    /// `CREATE_IN_PROGRESS` a discovery runs in and the `UPDATE_IN_PROGRESS` an
    /// update runs in — read off the stored status, so a status this slice does
    /// not mint yet counts as soon as something stores it.
    pub fn lifecycle_in_progress(&self) -> bool {
        self.status.ends_with("_IN_PROGRESS")
    }

    /// Whether a monitor deployment of this signal map is already running, so a
    /// second one — of any kind — cannot be started on top of it.
    pub fn monitor_deployment_in_progress(&self) -> bool {
        self.monitor_deployment
            .as_ref()
            .is_some_and(MonitorDeploymentRecord::in_progress)
    }

    /// The status of this signal map's latest monitor deployment, as
    /// `SignalMapSummary$MonitorDeploymentStatus` reports it: `NOT_DEPLOYED`
    /// while no deployment was ever started for it.
    pub fn monitor_deployment_status(&self) -> &str {
        self.monitor_deployment.as_ref().map_or(
            signal_map_monitor_deployment_status::NOT_DEPLOYED,
            |deployment| deployment.status.as_str(),
        )
    }

    /// Start a monitor deployment of this signal map, replacing the latest one:
    /// `StartMonitorDeployment` "initiates a deployment to deploy the LATEST
    /// monitor", so a signal map whose monitor was deployed before is deployed
    /// again rather than refused.
    ///
    /// The deployment starts in the IN_PROGRESS status of the kind the request
    /// asked for and comes due [`TRANSIENT_STATE_SPAN`] later, so a caller that
    /// polls straight after the call still observes it running. A deployment
    /// that has already completed is left in place until this one completes:
    /// `LastSuccessfulMonitorDeployment` is the latest SUCCESSFUL deployment,
    /// which a redeployment does not undo.
    pub fn begin_monitor_deployment(&mut self, dry_run: bool, now: SystemTime) {
        let kind = if dry_run {
            MonitorDeploymentKind::DryRun
        } else {
            MonitorDeploymentKind::Deploy
        };
        self.begin_deployment(kind, now);
    }

    /// Start a deployment to DELETE this signal map's monitor, replacing the
    /// latest deployment on the same terms as [`Self::begin_monitor_deployment`]
    /// replaces it: `StartDeleteMonitorDeployment` "initiates a deployment to
    /// delete the monitor of the specified signal map", which is the same
    /// asynchronous deployment of that monitor, so it runs in the deletion's own
    /// IN_PROGRESS status and comes due [`TRANSIENT_STATE_SPAN`] later.
    pub fn begin_delete_monitor_deployment(&mut self, now: SystemTime) {
        self.begin_deployment(MonitorDeploymentKind::Delete, now);
    }

    /// Start an update of this signal map: `StartUpdateSignalMap` "initiates an
    /// update for the specified signal map" and answers 202, so the signal map
    /// enters `UPDATE_IN_PROGRESS` and comes due [`TRANSIENT_STATE_SPAN`] later,
    /// where [`Self::settle`] finishes it — a caller that reads straight after
    /// the call still observes the update running.
    ///
    /// `discovered` is the graph a rediscovery found, held pending until the
    /// update completes, and `None` for an update that asked for none.
    pub fn begin_update(
        &mut self,
        discovered: Option<BTreeMap<String, MediaResource>>,
        now: SystemTime,
    ) {
        self.status = signal_map_status::UPDATE_IN_PROGRESS.to_string();
        self.update_due = Some(now + TRANSIENT_STATE_SPAN);
        self.pending_media_resource_map = discovered;
        self.modified_at = now;
    }

    /// Record `kind` as this signal map's latest monitor deployment, running and
    /// due [`TRANSIENT_STATE_SPAN`] from `now`. One writer for every kind, so no
    /// two of them can disagree about how a deployment starts.
    pub(super) fn begin_deployment(&mut self, kind: MonitorDeploymentKind, now: SystemTime) {
        self.monitor_deployment = Some(MonitorDeploymentRecord {
            kind,
            status: kind.in_progress_status().to_string(),
            details_uri: monitor_deployment_details_uri(&self.arn, &self.id),
            due: now + TRANSIENT_STATE_SPAN,
        });
        self.modified_at = now;
    }
}

/// The URI one signal map's monitor deployment reports as its `DetailsUri`: the
/// MediaLive console's page for that signal map, in the region the signal map's
/// own ARN names.
///
/// CHOSEN, not documented by AWS: `MonitorDeployment$DetailsUri` is only "URI
/// associated with a signal map's monitor deployment" and no AWS document gives
/// that URI a form, so this emulator serves one deterministic form of its own.
/// It is derived from STORED state — the signal map's ARN and its id — so every
/// read of a deployment reports the same URI, and it is never built out of the
/// request in flight.
pub(super) fn monitor_deployment_details_uri(arn: &str, id: &str) -> String {
    let region = arn_region(arn);
    format!(
        "https://{region}.console.aws.amazon.com/medialive/home\
         ?region={region}#!/signal-maps/{id}/monitor"
    )
}

/// The region segment of an ARN
/// (`arn:<partition>:<service>:<region>:<account-id>:<resource>`), empty for a
/// global service's ARNs, which carry no region.
pub(super) fn arn_region(arn: &str) -> &str {
    arn.split(':').nth(3).unwrap_or_default()
}

/// The description of one signal map, as `CreateSignalMap` and `GetSignalMap`
/// both report it: the model gives those operations structurally identical flat
/// outputs, so one projection feeds both through
/// [`signal_map_description_output`].
#[derive(Debug, Clone)]
pub struct SignalMapDescription {
    pub arn: Option<String>,
    pub cloud_watch_alarm_template_group_ids: Option<Vec<String>>,
    pub created_at: Option<::aws_smithy_types::DateTime>,
    pub description: Option<String>,
    pub discovery_entry_point_arn: Option<String>,
    pub error_message: Option<String>,
    pub event_bridge_rule_template_group_ids: Option<Vec<String>>,
    pub failed_media_resource_map: Option<BTreeMap<String, MediaResource>>,
    pub id: Option<String>,
    pub last_discovered_at: Option<::aws_smithy_types::DateTime>,
    pub last_successful_monitor_deployment: Option<SuccessfulMonitorDeployment>,
    pub media_resource_map: Option<BTreeMap<String, MediaResource>>,
    pub modified_at: Option<::aws_smithy_types::DateTime>,
    pub monitor_changes_pending_deployment: Option<bool>,
    pub monitor_deployment: Option<MonitorDeployment>,
    pub name: Option<String>,
    pub status: Option<String>,
    pub tags: Option<HashMap<String, String>>,
}

/// Build a new signal map record from a `CreateSignalMap` request's values and
/// the graph discovery found for its entry point.
///
/// The signal map starts in `CREATE_IN_PROGRESS` and owes `CREATE_COMPLETE`
/// once [`TRANSIENT_STATE_SPAN`] has passed, which is what the real service
/// does: creation is asynchronous and a caller waits on `Status`.
pub fn new_signal_map(
    id: String,
    arn: String,
    name: String,
    discovery_entry_point_arn: String,
    media_resource_map: BTreeMap<String, MediaResource>,
    now: SystemTime,
) -> SignalMapRecord {
    SignalMapRecord {
        id,
        arn,
        name,
        description: None,
        discovery_entry_point_arn,
        event_bridge_rule_template_group_ids: Vec::new(),
        cloud_watch_alarm_template_group_ids: Vec::new(),
        tags: HashMap::new(),
        created_at: now,
        modified_at: now,
        media_resource_map,
        status: signal_map_status::CREATE_IN_PROGRESS.to_string(),
        discovery_due: now + TRANSIENT_STATE_SPAN,
        // Nothing has updated a signal map nobody has updated yet, and nothing
        // is pending rediscovery for it: `StartUpdateSignalMap` is what starts
        // the first update.
        update_due: None,
        pending_media_resource_map: None,
        last_discovered_at: None,
        // No monitor has been deployed for a signal map nobody has deployed one
        // for: `StartMonitorDeployment` is what starts the first one.
        monitor_deployment: None,
        last_successful_monitor_deployment: None,
    }
}

/// The channel-to-input attachment edges of a whole store, in both directions:
/// each channel's attached inputs and, as the inverse, each input's attaching
/// channels.
///
/// Built by [`ComAmazonawsMedialiveMedialiveState::attachment_edges`] once per
/// discovery, so the walk reads the edges instead of re-deriving them per node.
#[derive(Debug, Default)]
pub(super) struct AttachmentEdges<'state> {
    /// A channel's ARN -> the inputs it attaches, in attachment order, once
    /// each, restricted to the ones the store still holds and has not deleted —
    /// discovery never claims a resource the store does not hold.
    pub(super) inputs_of: BTreeMap<&'state str, Vec<&'state InputRecord>>,
    /// An input's ARN -> the channels that attach it, in channel-id order.
    pub(super) channels_of: BTreeMap<&'state str, Vec<&'state ChannelRecord>>,
}

/// One media resource as a neighbour in a discovered media workflow graph: the
/// `Arn` the model requires of a `MediaResourceNeighbor`, plus the resource's
/// own name when it has one. One helper for every resource kind the graph holds,
/// so a channel neighbour and an input neighbour cannot be built two ways.
pub(super) fn neighbor(arn: &str, name: Option<&str>) -> MediaResourceNeighbor {
    MediaResourceNeighbor {
        arn: arn.to_string(),
        name: name.map(ToString::to_string),
    }
}
