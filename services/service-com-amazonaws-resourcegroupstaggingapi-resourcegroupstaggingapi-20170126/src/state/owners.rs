// SPDX-License-Identifier: CC0-1.0

//! The owning services the tagging API is a VIEW over.
//!
//! `TagResources` "appl[ies] one or more tags to the specified resources", and
//! the resources are not this service's: each one belongs to the service whose
//! namespace its ARN names, which is why AWS reports a per-resource
//! `FailureInfo` carrying "any valid error code returned by the Amazon Web
//! Services service that hosts the resource that the ARN key represents"
//! (`FailureInfo` documentation). An ARN is therefore RESOLVED against the
//! owning service's own state before a tag is written, and where that service
//! models the resource's tag set, that set — not a second copy here — is what
//! this API reads and writes. So for those resources a tag written here is
//! visible through the owning service's own `ListTagsFor*`, a tag written there
//! is visible through `GetResources`, and deleting the resource there removes it
//! from this API's index, with nothing to keep in step.
//!
//! That last part holds only where the owner MODELS the tag set. A resource the
//! owner resolves but models no tags for ([`Resolution::Present`] with no
//! `tags`: an EventBridge rule, a Step Functions execution) keeps its tags in
//! [`crate::state`]'s own store, which the owner's delete does not reach — so
//! `GetResources` goes on reporting that ARN until something tags or untags it
//! again. The alternative, dropping the entry the moment an owner answers
//! [`Resolution::Absent`], would make a READ delete state, which a reporting
//! operation may not do; the stale row is the lesser of the two, and it
//! disappears for every owner that models tags.
//!
//! # What "linked" means
//!
//! One emulator process holds every service's state in one engine, but a crate
//! can only read a state struct it names, so this module lists the owning
//! services this crate depends on. For an ARN in any OTHER namespace the answer
//! is [`Resolution::Unowned`]: the tagging API neither confirms nor denies the
//! resource and [`crate::state`]'s own store holds its tags, which is this
//! emulator's boundary rather than an AWS rule. Adding a service here is a
//! dependency in `Cargo.toml` plus one arm of [`visit`] and one of
//! [`tagged_resources`].

use std::collections::BTreeMap;
use std::time::SystemTime;

// The owning service crates, aliased to the role they play here. Each is
// declared `{ workspace = true }` in this crate's `Cargo.toml`, so the path the
// workspace resolves them at stays in one place.
use service_com_amazonaws_ecs_amazonec2containerservicev20141113 as owner_ecs;
use service_com_amazonaws_eventbridge_awsevents as owner_events;
use service_com_amazonaws_kms_trentservice as owner_kms;
use service_com_amazonaws_secretsmanager_secretsmanager as owner_secretsmanager;
use service_com_amazonaws_sfn_awsstepfunctions as owner_sfn;
use service_com_amazonaws_sqs_amazonsqs as owner_sqs;

use super::ParsedArn;
use crate::ops::types::FailureInfo;

/// Tag pairs in the one form this crate handles them in: key -> value, ordered,
/// so a projection of them is stable per read.
pub type Tags = BTreeMap<String, String>;

/// What the service that OWNS an ARN's namespace says about that ARN.
#[derive(Debug, Clone)]
pub enum Resolution {
    /// The owning service holds the resource. `tags` is `Some` where that
    /// service models the resource's own tag set — which is then the tag set
    /// this API reports and the one it writes — and `None` where it models
    /// none, leaving [`crate::state`]'s store as this resource's tag home.
    Present { tags: Option<Tags> },
    /// The owning service holds the resource, but the change would have left it
    /// carrying more tags than the documented limit, so NOTHING was written.
    /// The count is the one the change would have reached.
    TooManyTags(usize),
    /// The owning service serves this namespace and holds no such resource. The
    /// per-resource failure carries THAT service's not-found identity, which is
    /// what a client reading `FailedResourcesMap` sees from AWS.
    Absent(FailureInfo),
    /// No owning service linked into this crate serves this namespace.
    Unowned,
}

/// A tag write applied to the owning service's own tag map.
#[derive(Debug, Clone, Copy)]
pub enum Change<'a> {
    /// Upsert every pair: a key already on the resource is overwritten, every
    /// other key it carries is left alone — unless the merged key set would
    /// pass `max_tags`, which refuses the whole write for that resource so the
    /// check and the write share one acquisition of the owner's lock.
    Upsert { tags: &'a Tags, max_tags: usize },
    /// Remove exactly the named keys, leaving every other key alone.
    Remove(&'a [String]),
}

impl Change<'_> {
    /// Apply this change to a resource's tag map, or report the tag count it
    /// would have left the resource with when that is past the limit.
    pub fn apply(&self, tags: &mut Tags) -> Result<(), usize> {
        match self {
            Self::Upsert {
                tags: upserts,
                max_tags,
            } => {
                let merged = super::tag_count_after_merging(Some(tags), upserts.keys());
                if merged > *max_tags {
                    return Err(merged);
                }
                for (key, value) in upserts.iter() {
                    tags.insert(key.clone(), value.clone());
                }
            }
            Self::Remove(keys) => {
                for key in keys.iter() {
                    tags.remove(key);
                }
            }
        }
        Ok(())
    }
}

/// The per-resource failure for an ARN the owning service holds no resource for:
/// that service's own error code, with the 400 the tagging API reports for a
/// resource-level client fault.
fn absent(code: &str, message: String) -> Resolution {
    Resolution::Absent(FailureInfo {
        status_code: 400,
        error_code: Some(code.to_string()),
        error_message: Some(message),
    })
}

/// Read, and optionally change, a tag list whose entries carry an OPTIONAL key
/// and value — the `Tag` shape most services model. `$make` builds one of the
/// owning service's own tag records from a pair.
///
/// The list is edited IN PLACE: a record the change does not name keeps its
/// position and its exact stored value, so tagging one key never rewrites the
/// rest of the owning service's tag list.
macro_rules! option_tag_list {
    ($list:expr, $change:expr, $make:expr) => {{
        let mut pairs: Tags = option_tag_pairs!($list);
        if let Some(change) = $change {
            if let Err(would_hold) = change.apply(&mut pairs) {
                return Resolution::TooManyTags(would_hold);
            }
            match change {
                Change::Upsert { tags: upserts, .. } => {
                    for (key, value) in upserts.iter() {
                        match $list
                            .iter_mut()
                            .find(|tag| tag.key.as_deref() == Some(key.as_str()))
                        {
                            Some(held) => held.value = Some(value.clone()),
                            None => $list.push($make(key, value)),
                        }
                    }
                }
                Change::Remove(keys) => $list.retain(|tag| {
                    tag.key
                        .as_deref()
                        .is_none_or(|key| !keys.iter().any(|removed| removed == key))
                }),
            }
        }
        Resolution::Present { tags: Some(pairs) }
    }};
}

/// The [`Tags`] of a list of optional-key/optional-value tag records.
macro_rules! option_tag_pairs {
    ($list:expr) => {
        $list
            .iter()
            .filter_map(|tag| {
                tag.key
                    .clone()
                    .map(|key| (key, tag.value.clone().unwrap_or_default()))
            })
            .collect::<Tags>()
    };
}

/// The [`Tags`] of a list of tag records whose key and value are REQUIRED (the
/// `Tag` shape EventBridge models).
fn plain_tag_pairs(list: &[owner_events::ops::types::Tag]) -> Tags {
    list.iter()
        .map(|tag| (tag.key.clone(), tag.value.clone()))
        .collect()
}

/// Resolve `arn` against the service that owns its namespace, applying `change`
/// to that service's own tag map when one is supplied and the resource is there.
///
/// Locks exactly ONE owning service's state, for the length of this call: no
/// caller may hold [`crate::state::service_state`]'s guard across it, so the two
/// stores are never locked in two orders.
pub fn visit<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    match arn.service.as_str() {
        "sqs" => sqs(engine, arn, change),
        "ecs" => ecs(engine, arn, change),
        "events" => events(engine, arn, change),
        "kms" => kms(engine, arn, change),
        "secretsmanager" => secretsmanager(engine, arn, change),
        "states" => states(engine, arn, change),
        _ => Resolution::Unowned,
    }
}

/// Every resource an owning service itself holds tags for, as `(ARN, tags)`.
///
/// Only resources carrying at least one tag: "`GetResources` does not return
/// untagged resources." `region` is the serving region, needed for the one
/// resource whose ARN is derived rather than stored (an EventBridge event bus).
pub fn tagged_resources<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
) -> Vec<(String, Tags)> {
    let mut found: Vec<(String, Tags)> = Vec::new();
    let mut record = |arn: String, tags: Tags| {
        if !tags.is_empty() {
            found.push((arn, tags));
        }
    };

    if let Some(state) = owner_sqs::state::service_state(engine) {
        for queue in state.lock().queues.values() {
            record(queue.arn.clone(), queue.tags.clone());
        }
    }
    if let Some(state) = owner_ecs::state::service_state(engine) {
        let state = state.lock();
        for cluster in state.clusters.values() {
            record(cluster.arn.clone(), option_tag_pairs!(cluster.tags));
        }
        for provider in state.capacity_providers.values() {
            record(provider.arn.clone(), option_tag_pairs!(provider.tags));
        }
        for definition in state.task_definitions.values() {
            record(definition.arn.clone(), option_tag_pairs!(definition.tags));
        }
        for service in state.services.values() {
            record(service.arn.clone(), option_tag_pairs!(service.tags));
        }
        for task in state.tasks.values() {
            record(task.task_arn.clone(), option_tag_pairs!(task.tags));
        }
        for instance in state.container_instances.values() {
            record(instance.arn.clone(), option_tag_pairs!(instance.tags));
        }
    }
    if let Some(state) = owner_events::state::service_state(engine) {
        for bus in state.lock().buses.values() {
            record(
                owner_events::state::event_bus_arn(region, &bus.name),
                plain_tag_pairs(&bus.tags),
            );
        }
    }
    if let Some(state) = owner_kms::state::service_state(engine) {
        for key in state.lock().keys.values() {
            record(key.arn.clone(), key.tags.clone());
        }
    }
    if let Some(state) = owner_secretsmanager::state::service_state(engine) {
        let mut state = state.lock();
        // The one owner whose resources the CLOCK removes: a deletion that has
        // already fallen due (the end of a recovery window, or the short
        // asynchronous span a `ForceDeleteWithoutRecovery` deletion skips the
        // window for) was owed at this instant whether or not anybody called,
        // so it is materialised before this projection is read and a secret the
        // owning service would already have deleted is never reported. This is
        // the only write a read of this API performs, and it changes nothing a
        // later caller of that service would not have seen anyway.
        state.expire_due(SystemTime::now());
        for secret in state.secrets.values() {
            record(secret.arn.clone(), option_tag_pairs!(secret.tags));
        }
    }
    if let Some(state) = owner_sfn::state::service_state(engine) {
        for machine in state.lock().state_machines.values() {
            record(machine.arn.clone(), option_tag_pairs!(machine.tags));
        }
    }
    found
}

/// Amazon SQS: a queue, whose tags are the queue's own cost-allocation tags.
fn sqs<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    let Some(state) = owner_sqs::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    let mut state = state.lock();
    let Some(queue) = state.queues.values_mut().find(|queue| queue.arn == arn.arn) else {
        // `QueueDoesNotExist`, whose wire code is its `awsQueryError`:
        // `smithy_query(query="shape", shape="QueueDoesNotExist")` on the SQS
        // model reports code `AWS.SimpleQueueService.NonExistentQueue`, 400.
        return absent(
            "AWS.SimpleQueueService.NonExistentQueue",
            "The specified queue does not exist.".to_string(),
        );
    };
    if let Some(change) = change {
        if let Err(would_hold) = change.apply(&mut queue.tags) {
            return Resolution::TooManyTags(would_hold);
        }
    }
    Resolution::Present {
        tags: Some(queue.tags.clone()),
    }
}

/// Amazon ECS: clusters, capacity providers, task definitions, services, tasks
/// and container instances all carry their own `tags`.
///
/// The ARN is resolved against those records by the ECS crate's OWN resolver
/// ([`owner_ecs::state::TaggedResourceKind`] plus `resource_tags_mut`), so the
/// kinds this API can tag and the records it writes to are the ones that service
/// recognises, with no second copy of the resolution here to drift from it.
fn ecs<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    use owner_ecs::state::TaggedResourceKind;

    /// One ECS tag record from a pair.
    fn tag(key: &str, value: &str) -> owner_ecs::ops::types::Tag {
        owner_ecs::ops::types::Tag {
            key: Some(key.to_string()),
            value: Some(value.to_string()),
        }
    }

    let Some(state) = owner_ecs::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    let kind = TaggedResourceKind::from_arn(&arn.arn);
    let mut state = state.lock();
    if let Some(kind) = kind {
        if let Some(tags) = state.resource_tags_mut(kind, &arn.arn) {
            return option_tag_list!(tags, change, tag);
        }
    }
    // ECS's own `TagResource` models both refusals
    // (`smithy_query(query="operation", operation="TagResource")` on the ECS
    // model): the cluster-specific one for a cluster ARN, the generic one
    // otherwise.
    if kind == Some(TaggedResourceKind::Cluster) {
        absent(
            "ClusterNotFoundException",
            owner_ecs::state::CLUSTER_NOT_FOUND_MESSAGE.to_string(),
        )
    } else {
        absent(
            "ResourceNotFoundException",
            owner_ecs::state::resource_not_found_message(&arn.arn),
        )
    }
}

/// Amazon EventBridge: an event bus carries its own `tags`; a rule is resolved
/// but the emulator models no tag set for it, so the tagging store is its home.
fn events<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    use owner_events::ops::types::Tag;
    use owner_events::state::event_bus_arn;

    let Some(state) = owner_events::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    let mut state = state.lock();
    let names: Vec<String> = state.buses.keys().cloned().collect();
    for name in names {
        if event_bus_arn(&arn.region, &name) == arn.arn {
            let bus = state
                .buses
                .get_mut(&name)
                .expect("a bus named by the key just read from the same map");
            let mut pairs = plain_tag_pairs(&bus.tags);
            if let Some(change) = change {
                if let Err(would_hold) = change.apply(&mut pairs) {
                    return Resolution::TooManyTags(would_hold);
                }
                // In place, for the reason `option_tag_list!` gives.
                match change {
                    Change::Upsert { tags: upserts, .. } => {
                        for (key, value) in upserts.iter() {
                            match bus.tags.iter_mut().find(|tag| &tag.key == key) {
                                Some(held) => held.value = value.clone(),
                                None => bus.tags.push(Tag {
                                    key: key.clone(),
                                    value: value.clone(),
                                }),
                            }
                        }
                    }
                    Change::Remove(keys) => bus
                        .tags
                        .retain(|tag| !keys.iter().any(|removed| removed == &tag.key)),
                }
            }
            return Resolution::Present { tags: Some(pairs) };
        }
    }
    let holds_rule = state
        .buses
        .values()
        .any(|bus| bus.rules.values().any(|rule| rule.arn == arn.arn));
    if holds_rule {
        return Resolution::Present { tags: None };
    }
    absent(
        "ResourceNotFoundException",
        format!("The specified resource wasn't found: {}", arn.arn),
    )
}

/// Amazon KMS: a customer managed key, whose tags are the key's own.
fn kms<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    let Some(state) = owner_kms::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    let mut state = state.lock();
    let Some(key) = state.keys.values_mut().find(|key| key.arn == arn.arn) else {
        return absent(
            "NotFoundException",
            format!("Key '{}' does not exist", arn.arn),
        );
    };
    if let Some(change) = change {
        if let Err(would_hold) = change.apply(&mut key.tags) {
            return Resolution::TooManyTags(would_hold);
        }
    }
    Resolution::Present {
        tags: Some(key.tags.clone()),
    }
}

/// AWS Secrets Manager: a secret, whose tags are the secret's own.
fn secretsmanager<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    /// One Secrets Manager tag record from a pair.
    fn tag(key: &str, value: &str) -> owner_secretsmanager::state::StoredTag {
        owner_secretsmanager::state::StoredTag {
            key: Some(key.to_string()),
            value: Some(value.to_string()),
        }
    }

    let Some(state) = owner_secretsmanager::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    let mut state = state.lock();
    // As in `tagged_resources`: a deletion the clock has already made due is
    // materialised first, so an ARN whose secret the owning service would
    // already have purged resolves as absent rather than being tagged.
    state.expire_due(SystemTime::now());
    let Some(name) = state.arn_index.get(&arn.arn).cloned() else {
        return absent(
            "ResourceNotFoundException",
            "Secrets Manager can't find the specified secret.".to_string(),
        );
    };
    let Some(secret) = state.secrets.get_mut(&name) else {
        return absent(
            "ResourceNotFoundException",
            "Secrets Manager can't find the specified secret.".to_string(),
        );
    };
    option_tag_list!(&mut secret.tags, change, tag)
}

/// AWS Step Functions: a state machine, whose tags are the machine's own. An
/// execution is resolved but carries no tag set of its own.
fn states<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &ParsedArn,
    change: Option<&Change<'_>>,
) -> Resolution {
    /// One Step Functions tag record from a pair.
    fn tag(key: &str, value: &str) -> owner_sfn::ops::types::Tag {
        owner_sfn::ops::types::Tag {
            key: Some(key.to_string()),
            value: Some(value.to_string()),
        }
    }

    let Some(state) = owner_sfn::state::service_state(engine) else {
        return Resolution::Unowned;
    };
    let mut state = state.lock();
    if let Some(machine) = state.state_machines.get_mut(&arn.arn) {
        return option_tag_list!(&mut machine.tags, change, tag);
    }
    if state.executions.contains_key(&arn.arn) {
        return Resolution::Present { tags: None };
    }
    // Step Functions' own `ResourceNotFound`, whose message the service builds
    // from the request: "Resource not found: '<arn>'".
    absent(
        "ResourceNotFound",
        format!("Resource not found: '{}'", arn.arn),
    )
}
