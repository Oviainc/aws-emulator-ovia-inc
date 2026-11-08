// SPDX-License-Identifier: CC0-1.0

//! What one channel record computes about itself: how it is minted from a
//! `CreateChannel`, the clock-owed transitions its status walks, the destination
//! change an `UpdateChannel` applies, and the projections its readers report.
//!
//! Split out of the parent state module, which keeps to the RECORDS and the
//! service's fixed values.

use super::*;

impl Channel {
    /// Mint a channel of `cluster`: derive its identity from `suffix` (the
    /// registry's monotonic counter) and the cluster that holds it, and keep the
    /// submitted configuration whole.
    ///
    /// `now` is the creation instant and `input` the submitted configuration as
    /// `CreateChannel` deserialized it. The destination type is DERIVED from
    /// which destination configuration the request carried; the handler has
    /// already refused a request that carried both or neither, so a record can
    /// never describe a destination it does not hold.
    pub fn streaming(
        cluster: &Cluster,
        suffix: u64,
        now: SystemTime,
        input: CreateChannelInput,
    ) -> Self {
        let name = input.channel_name.as_str();
        // The registry's counter, as a 1-based ordinal; the SAME value seeds both
        // digests below, so a channel's id and its create operation's id follow
        // from one number.
        let ordinal = suffix + 1;
        let channel_id = uuid_v4_shaped_from_digest(&stable_digest(&format!(
            "{}:{}:channel:{name}:{ordinal}",
            ::engine::emulator_account_id(),
            cluster.arn
        )));
        let operation_id = uuid_v4_shaped_from_digest(&stable_digest(&format!(
            "{}:{}:channel-operation:{name}:{ordinal}",
            ::engine::emulator_account_id(),
            cluster.arn
        )));
        // The cluster's own resource id, so a channel ARN names the cluster it
        // belongs to the way an MSK sub-resource ARN does.
        let cluster_id = cluster.arn.split('/').next_back().unwrap_or_default();
        let destination_type = if input.iceberg_destination_configuration.is_some() {
            DESTINATION_TYPE_ICEBERG
        } else {
            DESTINATION_TYPE_S3
        };

        Self {
            arn: ::engine::emulator_arn(
                "kafka",
                &cluster.region,
                &format!("channel/{}/{cluster_id}/{name}/{channel_id}", cluster.name),
            ),
            name: input.channel_name,
            cluster_arn: cluster.arn.clone(),
            region: cluster.region.clone(),
            creation_time: now,
            active_due: now + CHANNEL_ACTIVATION_DELAY,
            status: CHANNEL_STATUS_CREATING.to_string(),
            removal_due: None,
            destination_type: destination_type.to_string(),
            encryption_configuration: input.encryption_configuration,
            iceberg_destination_configuration: input.iceberg_destination_configuration,
            s3_destination_configuration: input.s3_destination_configuration,
            topic_configuration_list: input.topic_configuration_list,
            logging_info: input.logging_info,
            tags: input.tags.unwrap_or_default().into_iter().collect(),
            cluster_operation_arn: ::engine::emulator_arn(
                "kafka",
                &cluster.region,
                &format!(
                    "cluster-operation/{}/{cluster_id}/{operation_id}",
                    cluster.name
                ),
            ),
            deletion_operation_arn: None,
            update_operation_arn: None,
            updates_accepted: 0,
        }
    }

    /// Apply the transition to `ACTIVE` once `now` has reached
    /// [`Self::active_due`]. Idempotent, and a no-op for every other status: only
    /// a transition the clock already owed is materialised here.
    ///
    /// The two non-terminal statuses a channel of this emulator leaves on a clock
    /// are `CREATING`, after the create, and `UPDATING`, after an accepted
    /// destination change — both of them end in `ACTIVE`, and both are owed at the
    /// instant the call that started them recorded. A channel that is `DELETING`
    /// never goes back to `ACTIVE`.
    pub fn settle(&mut self, now: SystemTime) {
        let settling =
            self.status == CHANNEL_STATUS_CREATING || self.status == CHANNEL_STATUS_UPDATING;
        if settling && now >= self.active_due {
            self.status = CHANNEL_STATUS_ACTIVE.to_string();
        }
    }

    /// Accept a destination change at `now`: write
    /// `data_freshness_in_seconds` onto the destination this channel was created
    /// with, move the channel to `UPDATING`, record the instant the clock owes it
    /// `ACTIVE` again, and mint the cluster operation carrying the change out —
    /// whose ARN is handed back, since that is what the caller polls.
    ///
    /// Only the member the request named is written: every other stored attribute
    /// of the channel — its name, its tags, its topic list, the rest of its
    /// destination configuration — is left exactly as it was, so an update of one
    /// member cannot rewrite another.
    pub fn begin_update(&mut self, now: SystemTime, data_freshness_in_seconds: i32) -> &str {
        // Each accepted update is its own operation, so the ordinal that seeds the
        // id advances with it: the seed is the channel's identity plus that ordinal,
        // which makes the ARN distinct from the create's, from the delete's and from
        // every earlier update's.
        self.updates_accepted += 1;
        self.update_operation_arn = Some(
            self.operation_arn_from(&format!(
                "channel-update-operation:{}",
                self.updates_accepted
            )),
        );
        // The update payload carries the member for ONE destination kind and the
        // record holds exactly one destination, so writing it wherever the record
        // carries it applies the change to the destination the channel actually
        // has; the handler has already refused an update naming the other kind.
        if let Some(destination) = self.s3_destination_configuration.as_mut() {
            destination.data_freshness_in_seconds = Some(data_freshness_in_seconds);
        }
        if let Some(destination) = self.iceberg_destination_configuration.as_mut() {
            destination.data_freshness_in_seconds = Some(data_freshness_in_seconds);
        }
        self.status = CHANNEL_STATUS_UPDATING.to_string();
        self.active_due = now + CHANNEL_UPDATE_DELAY;
        self.update_operation_arn.as_deref().unwrap_or_default()
    }

    /// Whether this channel is still part of the registry a caller at `now` sees:
    /// a channel whose removal the clock already owes is gone, whether or not the
    /// record has been dropped yet.
    pub fn is_live(&self, now: SystemTime) -> bool {
        self.removal_due.is_none_or(|due| now < due)
    }

    /// The ARN of a cluster operation of THIS channel, derived from `seed`.
    ///
    /// A channel's lifecycle operations all live under the same cluster as the create
    /// that minted it, so the ARN keeps that prefix and replaces only the operation
    /// id; `seed` is what makes the id distinct, since the digest is this channel's
    /// own identity plus it. The one derivation, so no two lifecycle calls of one
    /// channel can mint operation ARNs in two different ways.
    fn operation_arn_from(&self, seed: &str) -> String {
        let operation_id = uuid_v4_shaped_from_digest(&stable_digest(&format!(
            "{}:{}:{seed}",
            ::engine::emulator_account_id(),
            self.arn
        )));
        let prefix = self
            .cluster_operation_arn
            .rsplit_once('/')
            .map_or(self.cluster_operation_arn.as_str(), |(head, _)| head);
        format!("{prefix}/{operation_id}")
    }

    /// Accept this channel's deletion at `now`: move it to `DELETING`, record the
    /// instant the clock owes its removal, and mint the cluster operation carrying
    /// the delete out.
    ///
    /// Idempotent — a channel already `DELETING` keeps the removal instant and the
    /// operation ARN it was given, so repeating the call cannot postpone the
    /// removal or re-identify the operation.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        if self.removal_due.is_some() {
            return;
        }
        // Seeded so the delete's id is distinct from the create's and from every
        // update's, and stable across repeats of this call.
        self.deletion_operation_arn = Some(self.operation_arn_from("channel-delete-operation"));
        self.status = CHANNEL_STATUS_DELETING.to_string();
        self.removal_due = Some(now + CHANNEL_DELETION_DELAY);
    }

    /// The cluster operation a reader reports as this channel's in-flight one
    /// (`DescribeChannelResponse$ClusterOperationArn`, "the ARN of the in-flight
    /// cluster operation"): the delete once one has been accepted, otherwise the
    /// most recent accepted update, otherwise the create that minted the channel.
    pub fn in_flight_cluster_operation_arn(&self) -> &str {
        self.deletion_operation_arn
            .as_deref()
            .or(self.update_operation_arn.as_deref())
            .unwrap_or(&self.cluster_operation_arn)
    }

    /// The channel as `DescribeChannel` reports it. Kept next to the record for
    /// the same reason [`Cluster::to_cluster_info`] is: every reader of a channel
    /// renders the one stored record the same way.
    pub fn to_describe_output(&self) -> DescribeChannelOutput {
        DescribeChannelOutput {
            channel_arn: self.arn.clone(),
            channel_name: self.name.clone(),
            encryption_configuration: self.encryption_configuration.clone(),
            iceberg_destination_configuration: self.iceberg_destination_configuration.clone(),
            s3_destination_configuration: self.s3_destination_configuration.clone(),
            status: self.status.clone(),
            destination_type: self.destination_type.clone(),
            creation_time: ::aws_smithy_types::DateTime::from(self.creation_time),
            topic_configuration_list: self.topic_configuration_list.clone(),
            logging_info: self.logging_info.clone(),
            // `ChannelStateInfo` reports why a channel is in a state that needs
            // explaining; nothing has gone wrong with a channel this emulator
            // creates, so there is none to report.
            state_info: None,
            cluster_operation_arn: Some(self.in_flight_cluster_operation_arn().to_string()),
            tags: Some(self.tags.clone().into_iter().collect()),
        }
    }

    /// The channel as `ListChannels` summarises it, from the SAME stored record
    /// `DescribeChannel` reports: a listing and a read of one channel can only
    /// agree if they render the same fields of one record.
    ///
    /// `ClusterOperationArn` is "the ARN of the in-flight cluster operation.
    /// Returned only while the channel is in CREATING, UPDATING, or DELETING"
    /// (`ChannelInfo$ClusterOperationArn`), so a channel whose provisioning has
    /// settled reports none: there is no operation in flight on it.
    pub fn to_channel_info(&self) -> ChannelInfo {
        ChannelInfo {
            channel_arn: self.arn.clone(),
            channel_name: self.name.clone(),
            status: self.status.clone(),
            creation_time: ::aws_smithy_types::DateTime::from(self.creation_time),
            destination_type: self.destination_type.clone(),
            cluster_operation_arn: (self.status != CHANNEL_STATUS_ACTIVE)
                .then(|| self.in_flight_cluster_operation_arn().to_string()),
        }
    }

    /// Whether this channel streams the topic called `name`.
    ///
    /// A channel names its source topics by ARN, and a topic's NAME is the final
    /// segment of that ARN, so the comparison is made on that segment — which is
    /// what `ListChannelsRequest$TopicNameFilter` ("Filters results to channels
    /// whose topic name matches the specified value") is matched against.
    pub fn streams_topic_named(&self, name: &str) -> bool {
        self.topic_configuration_list.iter().any(|configuration| {
            configuration
                .topic_arn
                .split('/')
                .next_back()
                .is_some_and(|topic| topic == name)
        })
    }
}
