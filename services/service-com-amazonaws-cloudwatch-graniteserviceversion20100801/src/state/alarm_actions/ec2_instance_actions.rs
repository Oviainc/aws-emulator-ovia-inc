// SPDX-License-Identifier: CC0-1.0

//! The EC2 INSTANCE actions an alarm may carry, and what executing one does.
//!
//! "Valid values: EC2 actions: arn:aws:automate:region:ec2:stop
//! arn:aws:automate:region:ec2:terminate arn:aws:automate:region:ec2:reboot
//! arn:aws:automate:region:ec2:recover
//! arn:aws:swf:region:account-id:action/actions/AWS_EC2.InstanceId.Stop/1.0 …"
//! (`PutMetricAlarmInput$AlarmActions`, and the same for `OKActions` and
//! `InsufficientDataActions`) — four actions, each written in two forms, and each one an
//! operation of Amazon EC2 rather than a message to a target. So an alarm whose transition
//! fires one of them has that EC2 operation performed on the instance the alarm watches:
//! this module is the one place that reads WHICH action an ARN names ([`of_action_arn`],
//! which the alarm writers read the same ARNs with) and the one place that performs it
//! ([`execute`]).
//!
//! WHICH instance is read off the alarm's own metric: an EC2 instance action is accepted only
//! on an alarm every one of whose watched metrics is dimensioned by
//! [`INSTANCE_ID_DIMENSION`] (`crate::ops::put_metric_alarm`), so the instance the action acts
//! on is the one that dimension names ([`watched_instance_id`]) and never a value from the
//! request that fired the transition.
//!
//! EC2 is reached the way any other service is reached from here: over the AWS SDK driven on
//! the in-process callback ([`smithy_server_runtime::OperationContext::callback`]), in the
//! Region the action's own ARN names — which is the alarm's own Region, since an action
//! naming another one is refused when the alarm is written. Whether the instance exists, and
//! whether it is in a state the action can be carried out in, is EC2's answer to give: what
//! it says is what the alarm's history records for the execution
//! ([`super::action_history_item`]).

use crate::ops::types::{Dimension, MetricAlarm};

/// The dimension that names the EC2 instance an alarm monitors.
///
/// The one definition in this crate: the precondition on writing an EC2 instance action reads
/// it ([`instance_of_dimensions`]) and so does the execution of one.
pub const INSTANCE_ID_DIMENSION: &str = "InstanceId";

/// The ARN vendor of the `automate` form of an EC2 instance action.
const AUTOMATE_ARN_VENDOR: &str = "automate";

/// EC2's own namespace, which the `automate` form carries where an ARN usually carries an
/// account.
const EC2_ARN_NAMESPACE: &str = "ec2";

/// The ARN vendor of the Simple Workflow form.
const SWF_ARN_VENDOR: &str = "swf";

/// The resource prefix of the Simple Workflow form, from the same documentation.
const SWF_EC2_ACTION_PREFIX: &str = "action/actions/AWS_EC2.InstanceId.";

/// The version suffix that form's resource ends with.
const SWF_EC2_ACTION_SUFFIX: &str = "/1.0";

/// One of the four EC2 instance actions an alarm action list may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ec2InstanceAction {
    /// `…:ec2:stop` — the instance is stopped.
    Stop,
    /// `…:ec2:terminate` — the instance is terminated.
    Terminate,
    /// `…:ec2:reboot` — the instance's operating system is restarted.
    Reboot,
    /// `…:ec2:recover` — the instance is recovered onto healthy hardware. "The instance is
    /// migrated during an instance reboot, and any data that is in-memory is lost", and "a
    /// recovered instance is identical to the original instance, including the instance ID"
    /// (the recover action's own documentation), so what a client observes of it is the
    /// restart of the same instance.
    Recover,
}

/// The four actions beside the verb each is written with, which is both what an ARN is read
/// against and what one is reported by.
const EC2_INSTANCE_ACTIONS: [(&str, Ec2InstanceAction); 4] = [
    ("stop", Ec2InstanceAction::Stop),
    ("terminate", Ec2InstanceAction::Terminate),
    ("reboot", Ec2InstanceAction::Reboot),
    ("recover", Ec2InstanceAction::Recover),
];

impl Ec2InstanceAction {
    /// The action `verb` names, or `None` when it names none of the four.
    fn of_verb(verb: &str) -> Option<Self> {
        EC2_INSTANCE_ACTIONS
            .iter()
            .find(|(known, _)| verb.eq_ignore_ascii_case(known))
            .map(|(_, action)| *action)
    }
}

/// Which EC2 instance action `arn` names, or `None` when it names none.
///
/// The partition and region segments are not matched: an EC2 instance action is one whatever
/// partition or region it names, and an action in another region is still an EC2 instance
/// action (the alarm writers refuse it for the same reason). Any other ARN — an SNS topic, an
/// Auto Scaling policy, an SSM OpsItem — is not an EC2 instance action at all.
#[must_use]
pub fn of_action_arn(arn: &str) -> Option<Ec2InstanceAction> {
    named(arn).map(|named| named.action)
}

/// The action `arn` names beside the REGION field it carries, which is where that action is
/// performed.
struct Named<'arn> {
    action: Ec2InstanceAction,
    region: &'arn str,
}

/// `arn` read as an EC2 instance action: which action, and the region segment it names.
fn named(arn: &str) -> Option<Named<'_>> {
    // `arn:<partition>:<vendor>:<region>:<account>:<resource>`, resource last so a resource
    // carrying `:` stays whole.
    let mut segments = arn.splitn(6, ':');
    if segments.next() != Some("arn") {
        return None;
    }
    let _partition = segments.next();
    let vendor = segments.next().unwrap_or_default();
    let region = segments.next().unwrap_or_default();
    let account = segments.next().unwrap_or_default();
    let resource = segments.next().unwrap_or_default();
    let action = match vendor {
        AUTOMATE_ARN_VENDOR if account == EC2_ARN_NAMESPACE => Ec2InstanceAction::of_verb(resource),
        SWF_ARN_VENDOR => resource
            .strip_prefix(SWF_EC2_ACTION_PREFIX)
            .and_then(|rest| rest.strip_suffix(SWF_EC2_ACTION_SUFFIX))
            .and_then(Ec2InstanceAction::of_verb),
        _ => None,
    }?;
    Some(Named { action, region })
}

/// The instance `dimensions` names: the value of its [`INSTANCE_ID_DIMENSION`] dimension.
///
/// A dimension is `Name`/`Value`, both required, so an empty value names no instance — and a
/// list that carries no such dimension names none either, which is what the precondition on
/// writing an EC2 instance action reads.
#[must_use]
pub fn instance_of_dimensions(dimensions: Option<&[Dimension]>) -> Option<&str> {
    dimensions?
        .iter()
        .find(|dimension| dimension.name == INSTANCE_ID_DIMENSION && !dimension.value.is_empty())
        .map(|dimension| dimension.value.as_str())
}

/// The instance the alarm `alarm` watches, as its own metric identity names it.
///
/// An alarm over ONE metric is dimensioned by the request's own `Dimensions` ("The dimensions
/// for the metric specified in MetricName"), so the instance is the one those name. An alarm
/// configured with a query ARRAY watches the query that returns data
/// ([`crate::state::alarm_evaluation::watched_query`]), so that query's metric is asked first
/// and the other retrieving queries after it — a watched query that is an expression retrieves
/// no metric of its own, and the array's metrics are all per-instance for the alarm to carry
/// an EC2 instance action at all.
#[must_use]
pub fn watched_instance_id(alarm: &MetricAlarm) -> Option<&str> {
    let Some(queries) = alarm.metrics.as_deref() else {
        return instance_of_dimensions(alarm.dimensions.as_deref());
    };
    let watched =
        crate::state::alarm_evaluation::watched_query(queries, alarm.threshold_metric_id.as_deref());
    watched
        .into_iter()
        .chain(queries.iter())
        .filter_map(|query| query.metric_stat.as_ref())
        .find_map(|stat| instance_of_dimensions(stat.metric.dimensions.as_deref()))
}

/// Execute the EC2 instance action `arn` names against `instance_id`, and answer the failure
/// it failed with — `None` when EC2 carried it out.
///
/// Each action is the EC2 operation of the same name: the stop and the terminate move the
/// instance's lifecycle, while the reboot and the recover restart its operating system and
/// leave the instance where it is. Nothing is invented for an action that could not be
/// carried out: what EC2 said is what the alarm's history records, and an action with no
/// instance to act on is recorded as the failure it was.
pub async fn execute(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    arn: &str,
    instance_id: Option<&str>,
) -> Option<String> {
    let named = named(arn)?;
    // The Region ends up in the endpoint URL this hop is addressed on, so it is read under
    // the Region grammar every model states for the field rather than as arbitrary text.
    let Some(region) = ::service_common::arn::region_label(named.region) else {
        return Some(format!(
            "The action {arn} names no Region in which an EC2 instance action can be executed"
        ));
    };
    // An EC2 instance action is accepted only on an alarm that monitors an instance, so this
    // is the alarm whose watched metric no longer names one: nothing is acted on, and the
    // execution is recorded as having failed for that reason.
    let Some(instance_id) = instance_id else {
        return Some(format!(
            "The alarm's metric is not dimensioned by {INSTANCE_ID_DIMENSION}, so the action \
             {arn} names no instance to execute against"
        ));
    };
    let client = ec2_client(callback, region);
    match named.action {
        Ec2InstanceAction::Stop => client
            .stop_instances()
            .instance_ids(instance_id)
            .send()
            .await
            .err()
            .map(|error| super::action_failure(&error)),
        Ec2InstanceAction::Terminate => client
            .terminate_instances()
            .instance_ids(instance_id)
            .send()
            .await
            .err()
            .map(|error| super::action_failure(&error)),
        // A recover restarts the same instance on other hardware ("the instance is migrated
        // during an instance reboot"), which is the reboot EC2 performs for the instance it
        // owns.
        Ec2InstanceAction::Reboot | Ec2InstanceAction::Recover => client
            .reboot_instances()
            .instance_ids(instance_id)
            .send()
            .await
            .err()
            .map(|error| super::action_failure(&error)),
    }
}

/// An EC2 client over the in-process callback into this same emulator, addressed in `region`.
fn ec2_client(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_ec2::Client {
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The same goes for caching the identity: the credentials are handed over
        // directly.
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_ec2::Client::from_conf(config)
}
