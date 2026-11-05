// SPDX-License-Identifier: CC0-1.0

//! What one [`Replicator`](super::Replicator) record computes about itself: how a
//! create mints it, the state the clock has it in, and the shapes its readers
//! report.
//!
//! Split out of the parent state module, which keeps to the RECORDS and the
//! service's fixed values; the code here is unchanged.

use super::*;

impl Replicator {
    /// Mint a replicator from the submitted configuration.
    ///
    /// `region` is the caller's signing region, `suffix` the registry's monotonic
    /// counter (used as the 1-based ordinal `suffix + 1`, so one value seeds the
    /// whole identity), `now` the creation instant, `input` the submitted
    /// configuration as `CreateReplicator` deserialized it, and `failure` the
    /// provisioning failure the setup already carries (see [`Self::failure`]),
    /// which the clock turns into [`REPLICATOR_STATE_FAILED`].
    ///
    /// The configuration is stored with the service's own defaults APPLIED
    /// ([`applied_kafka_cluster`], [`applied_replication_info`]), so a member the
    /// request left out is reported from the record like any other instead of
    /// being absent from every reader.
    pub fn provisioned(
        region: &str,
        suffix: u64,
        now: SystemTime,
        input: CreateReplicatorInput,
        failure: Option<ReplicationStateInfo>,
    ) -> Self {
        let name = input.replicator_name.as_str();
        let ordinal = suffix + 1;
        let digest = stable_digest(&format!(
            "{}:{region}:replicator:{name}:{ordinal}",
            ::engine::emulator_account_id()
        ));
        let arn = ::engine::emulator_arn(
            "kafka",
            region,
            &format!(
                "replicator/{name}/{}-{ordinal}",
                uuid_v4_shaped_from_digest(&digest)
            ),
        );
        // CAPPED, not trusted: the handler refuses an over-size request with the
        // modelled `BadRequestException`, but this constructor must not let any
        // caller of this module store a record whose size — and so the work of
        // every later read of it — comes from an unchecked request value.
        let kafka_clusters = input
            .kafka_clusters
            .into_iter()
            .take(MAX_CLUSTERS_PER_REPLICATOR)
            .map(|cluster| ReplicatorCluster {
                // UNREACHABLE empty: the handler refuses an entry that identifies
                // no cluster, so every stored entry carries the alias its MSK ARN
                // or its Apache cluster id gives it.
                alias: kafka_cluster_alias(&cluster).unwrap_or_default(),
                cluster: applied_kafka_cluster(cluster),
            })
            .collect();
        // Capped for the same reason as the cluster list above.
        let replication_info: Vec<ReplicationInfo> = input
            .replication_info_list
            .into_iter()
            .take(MAX_FLOWS_PER_REPLICATOR)
            .map(applied_replication_info)
            .collect();
        Self {
            current_version: current_version_token(&arn, 1),
            revision: 1,
            resource_arn: arn.clone(),
            arn,
            name: input.replicator_name,
            region: region.to_string(),
            description: input.description,
            service_execution_role_arn: input.service_execution_role_arn,
            creation_time: now,
            running_due: now + REPLICATOR_ACTIVATION_DELAY,
            state: REPLICATOR_STATE_CREATING.to_string(),
            failure,
            // A replicator created here is the replicator itself, never the
            // read-only reference AWS surfaces in the OTHER region of a
            // cross-region setup.
            is_replicator_reference: false,
            kafka_clusters,
            // Each flow's patterns are compiled HERE, once, so no later read of
            // this record compiles a pattern a request supplied.
            topic_filters: replication_info
                .iter()
                .map(|info| TopicFilter::of(&info.topic_replication))
                .collect(),
            replication_info,
            log_delivery: input.log_delivery,
            // The handler already refuses an over-tagged request, so this cap is
            // an unreachable backstop — and it takes the surviving tags in KEY
            // order out of an ordered map, so if it ever did fire the subset it
            // keeps is deterministic rather than whatever order the request map
            // happened to iterate in.
            tags: input
                .tags
                .unwrap_or_default()
                .into_iter()
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .take(MAX_TAGS_PER_REPLICATOR)
                .collect(),
        }
    }

    /// Apply the transition the clock owes this replicator once `now` has reached
    /// [`Self::running_due`] — out of `CREATING` after the create and out of
    /// `UPDATING` after an accepted change, the two non-terminal states a
    /// replicator of this registry is reported in. A replicator whose
    /// provisioning failed ([`Self::failure`]) is owed `FAILED` rather than
    /// `RUNNING`: the clusters it was created over did not resolve, so it never
    /// starts replicating. Idempotent, and a no-op for every other state: only a
    /// transition the clock already owed is materialised here.
    pub fn settle(&mut self, now: SystemTime) {
        let settling = self.state == REPLICATOR_STATE_CREATING
            || self.state == REPLICATOR_STATE_UPDATING;
        if settling && now >= self.running_due {
            self.state = match self.failure {
                Some(_) => REPLICATOR_STATE_FAILED.to_string(),
                None => REPLICATOR_STATE_RUNNING.to_string(),
            };
        }
    }

    /// The `StateInfo` a reader reports for this replicator: the provisioning
    /// failure, once the clock has moved the replicator into
    /// [`REPLICATOR_STATE_FAILED`]. A state that explains itself — `CREATING`,
    /// `RUNNING`, `UPDATING`, `DELETING` — carries none.
    pub fn state_info(&self) -> Option<ReplicationStateInfo> {
        if self.state == REPLICATOR_STATE_FAILED {
            return self.failure.clone();
        }
        None
    }

    /// The instant this replicator's flows start replicating: the instant the
    /// clock first owes the replicator `RUNNING`.
    ///
    /// Read off the creation instant rather than [`Self::running_due`], which a
    /// later accepted update moves forward: an update rolls out while the flows
    /// the replicator already runs keep running, so a configuration change does
    /// not withdraw what has already been replicated.
    pub fn replication_start(&self) -> SystemTime {
        self.creation_time + REPLICATOR_ACTIVATION_DELAY
    }

    /// Whether this replicator is replicating at `now`: from
    /// [`Self::replication_start`] on, which is exactly when a reader reports it
    /// `RUNNING`. A replicator whose provisioning failed never replicates
    /// anything, whatever the clock says. A deleted replicator leaves the
    /// registry, so no caller can ask this about one.
    pub fn replicates_at(&self, now: SystemTime) -> bool {
        self.failure.is_none() && now >= self.replication_start()
    }

    /// The Amazon MSK cluster ARNs this replicator replicates INTO, once each:
    /// what the per-target index maps to this replicator, so a reader of one
    /// cluster's topics finds the replicators that feed it.
    ///
    /// A flow whose target is a self-managed Apache Kafka cluster names no ARN
    /// here: that cluster's topics live on the cluster itself, which this
    /// emulator does not hold.
    pub fn target_cluster_arns(&self) -> BTreeSet<String> {
        self.replication_info
            .iter()
            .filter_map(|info| info.target_kafka_cluster_arn.clone())
            .collect()
    }

    /// The position in [`Self::replication_info`] of the replication running
    /// between the clusters the aliases `source` and `target` name, or `None`
    /// when this replicator configures no replication for that ordered pair.
    ///
    /// A replication is addressed by the ALIASES of its two ends — the same way
    /// every reader reports it — so an end given as an Amazon MSK cluster ARN and
    /// the same end given as an Apache Kafka cluster id resolve through one rule,
    /// [`flow_cluster_alias`]. The pair is ORDERED: the aliases of a configured
    /// replication named the other way round address no replication.
    pub fn replication_index_of(&self, source: &str, target: &str) -> Option<usize> {
        self.replication_info.iter().position(|info| {
            let configured_source = flow_cluster_alias(
                info.source_kafka_cluster_arn.as_deref(),
                info.source_kafka_cluster_id.as_deref(),
            );
            let configured_target = flow_cluster_alias(
                info.target_kafka_cluster_arn.as_deref(),
                info.target_kafka_cluster_id.as_deref(),
            );
            configured_source.is_some_and(|alias| alias == source)
                && configured_target.is_some_and(|alias| alias == target)
        })
    }

    /// Fold an accepted replication-info update into the stored record: replace
    /// the members the update shapes carry on the replication at `index`, take
    /// the submitted log delivery when the request carried one, bump the
    /// replicator's revision — and so its `CurrentVersion` token — and move it to
    /// `UPDATING` with the instant the clock owes it `RUNNING` again.
    ///
    /// `TopicReplicationUpdate` and `ConsumerGroupReplicationUpdate` model every
    /// member they carry as required, so each one SETS that member of the stored
    /// configuration; the members those shapes do not model (a topic
    /// replication's starting position and topic-name configuration, a consumer
    /// group replication's offset sync mode) are not part of the update and keep
    /// the values the record already holds.
    ///
    /// A no-op for an `index` the record does not hold, so a caller of this
    /// module cannot change a replicator's version without changing its
    /// configuration.
    pub fn apply_replication_update(
        &mut self,
        index: usize,
        now: SystemTime,
        topic_replication: Option<TopicReplicationUpdate>,
        consumer_group_replication: Option<ConsumerGroupReplicationUpdate>,
        log_delivery: Option<LogDelivery>,
    ) {
        let Some(info) = self.replication_info.get_mut(index) else {
            return;
        };
        if let Some(update) = topic_replication {
            let stored = &mut info.topic_replication;
            stored.copy_access_control_lists_for_topics =
                Some(update.copy_access_control_lists_for_topics);
            stored.copy_topic_configurations = Some(update.copy_topic_configurations);
            stored.detect_and_copy_new_topics = Some(update.detect_and_copy_new_topics);
            stored.topics_to_exclude = Some(update.topics_to_exclude);
            stored.topics_to_replicate = update.topics_to_replicate;
        }
        if let Some(update) = consumer_group_replication {
            let stored = &mut info.consumer_group_replication;
            stored.consumer_groups_to_exclude = Some(update.consumer_groups_to_exclude);
            stored.consumer_groups_to_replicate = update.consumer_groups_to_replicate;
            stored.detect_and_copy_new_consumer_groups =
                Some(update.detect_and_copy_new_consumer_groups);
            stored.synchronise_consumer_group_offsets =
                Some(update.synchronise_consumer_group_offsets);
        }
        if log_delivery.is_some() {
            self.log_delivery = log_delivery;
        }
        // The filter is DERIVED from the configuration just written, so it is
        // rebuilt here rather than left reporting the patterns the update
        // replaced — and rebuilt for the one replication that changed.
        if let Some(info) = self.replication_info.get(index) {
            let filter = TopicFilter::of(&info.topic_replication);
            match self.topic_filters.get_mut(index) {
                Some(stored) => *stored = filter,
                // Unreachable: the filter list is built from the replication list
                // and the two are only ever set together. Grown rather than
                // dropped, so the replication at `index` keeps a filter.
                None => {
                    self.topic_filters.resize_with(index, TopicFilter::default);
                    self.topic_filters.push(filter);
                }
            }
        }
        self.revision += 1;
        self.current_version = current_version_token(&self.arn, self.revision);
        self.state = REPLICATOR_STATE_UPDATING.to_string();
        self.running_due = now + REPLICATOR_ACTIVATION_DELAY;
    }

    /// The compiled topic filter of the replication at `index`, or `None` when
    /// the record holds no such replication.
    pub fn topic_filter(&self, index: usize) -> Option<&TopicFilter> {
        self.topic_filters.get(index)
    }

    /// The alias of the stored cluster an endpoint of a `ReplicationInfo` names,
    /// or `None` when the setup holds no such cluster.
    ///
    /// A single lookup, linear in the cluster list. A reader resolving EVERY
    /// replication builds [`Self::alias_index`] once instead, so its work is
    /// `O(n + m)` in the replications and the clusters rather than their product.
    pub fn alias_of(&self, arn: Option<&str>, id: Option<&str>) -> Option<String> {
        self.kafka_clusters
            .iter()
            .find(|cluster| cluster.names(arn, id))
            .map(|cluster| cluster.alias.clone())
    }

    /// A lookup from the way a `ReplicationInfo` can name a cluster — by ARN for
    /// an Amazon MSK cluster (`true`) or by id for an Apache Kafka one (`false`)
    /// — to that cluster's alias.
    ///
    /// Built ONCE per rendering, so resolving every replication costs `O(n + m)`
    /// rather than one linear scan of the clusters per replication endpoint. The
    /// first cluster to claim a key keeps it, which is the same cluster
    /// [`Self::alias_of`]'s `find` would have returned.
    pub(super) fn alias_index(&self) -> HashMap<(bool, &str), &str> {
        let mut index = HashMap::with_capacity(self.kafka_clusters.len());
        for entry in &self.kafka_clusters {
            if let Some(msk) = entry.cluster.amazon_msk_cluster.as_ref() {
                index
                    .entry((true, msk.msk_cluster_arn.as_str()))
                    .or_insert(entry.alias.as_str());
            }
            if let Some(apache) = entry.cluster.apache_kafka_cluster.as_ref() {
                index
                    .entry((false, apache.apache_kafka_cluster_id.as_str()))
                    .or_insert(entry.alias.as_str());
            }
        }
        index
    }

    /// The replicator as the `ReplicatorSummary` `ListReplicators` reports.
    ///
    /// Each replication is reported by the ALIASES of its endpoints, resolved
    /// against this replicator's own cluster list — the summary shape carries no
    /// ARN or id member, so the aliases are derived from the stored setup rather
    /// than echoed from it.
    pub fn to_replicator_summary(&self) -> ReplicatorSummary {
        let aliases = self.alias_index();
        ReplicatorSummary {
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            current_version: Some(self.current_version.clone()),
            is_replicator_reference: Some(self.is_replicator_reference),
            kafka_clusters_summary: Some(
                self.kafka_clusters
                    .iter()
                    .map(ReplicatorCluster::to_summary)
                    .collect(),
            ),
            replication_info_summary_list: Some(
                self.replication_info
                    .iter()
                    .map(|info| ReplicationInfoSummary {
                        source_kafka_cluster_alias: indexed_alias(
                            &aliases,
                            info.source_kafka_cluster_arn.as_deref(),
                            info.source_kafka_cluster_id.as_deref(),
                        ),
                        target_kafka_cluster_alias: indexed_alias(
                            &aliases,
                            info.target_kafka_cluster_arn.as_deref(),
                            info.target_kafka_cluster_id.as_deref(),
                        ),
                    })
                    .collect(),
            ),
            replicator_arn: Some(self.arn.clone()),
            replicator_name: Some(self.name.clone()),
            replicator_resource_arn: Some(self.resource_arn.clone()),
            replicator_state: Some(self.state.clone()),
        }
    }

    /// The replicator's Kafka clusters as `DescribeReplicator` reports them: the
    /// same stored records [`Self::to_replicator_summary`] summarises, rendered
    /// into the richer description shape.
    pub fn kafka_cluster_descriptions(&self) -> Vec<KafkaClusterDescription> {
        self.kafka_clusters
            .iter()
            .map(ReplicatorCluster::to_description)
            .collect()
    }

    /// The replicator's replications as `DescribeReplicator` reports them: the
    /// stored configuration, with each end named by the ALIAS resolved against
    /// this replicator's own cluster list — through the SAME index
    /// [`Self::to_replicator_summary`] uses, so the two readers cannot disagree
    /// about which cluster an end names, and neither does work proportional to
    /// the product of the two lists.
    pub fn replication_info_descriptions(&self) -> Vec<ReplicationInfoDescription> {
        let aliases = self.alias_index();
        self.replication_info
            .iter()
            .map(|info| ReplicationInfoDescription {
                consumer_group_replication: Some(info.consumer_group_replication.clone()),
                source_kafka_cluster_alias: indexed_alias(
                    &aliases,
                    info.source_kafka_cluster_arn.as_deref(),
                    info.source_kafka_cluster_id.as_deref(),
                ),
                target_compression_type: Some(info.target_compression_type.clone()),
                target_kafka_cluster_alias: indexed_alias(
                    &aliases,
                    info.target_kafka_cluster_arn.as_deref(),
                    info.target_kafka_cluster_id.as_deref(),
                ),
                topic_replication: Some(info.topic_replication.clone()),
            })
            .collect()
    }
}
