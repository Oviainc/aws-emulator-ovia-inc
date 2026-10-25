// SPDX-License-Identifier: CC0-1.0

//! The ACTIONS an alarm state transition fires, and how they are delivered.
//!
//! "The actions to execute when this alarm transitions to the ALARM state from any other
//! state. Each action is specified as an Amazon Resource Name (ARN). Valid Values: Amazon SNS
//! actions: arn:aws:sns:region:account-id:sns-topic-name […]"
//! (`PutMetricAlarmInput$AlarmActions`, and the same for `OKActions` and
//! `InsufficientDataActions`) — so when an alarm's `StateValue` changes, the ARNs configured
//! for the state it moved TO are executed, and an SNS topic ARN among them receives the
//! alarm's notification. `ActionsEnabled` is the switch: "Indicates whether actions should be
//! executed during any changes to the alarm state" (`MetricAlarm$ActionsEnabled`).
//!
//! The notification is the JSON document CloudWatch publishes for an alarm state change: the
//! alarm's identity, the state it moved to and the state it moved from, the reason, and the
//! configuration that was evaluated. A client reads `NewStateValue` out of it, which is why
//! the envelope is built here, once, for every transition whichever call materialised it.
//!
//! An action is reached the way any emulated service reaches another: through the in-process
//! callback ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no
//! other service crate. Delivery is asynchronous in AWS and is not part of any operation's
//! answer, so an action that cannot be reached is logged and recorded in the alarm's history
//! as a failed action, and changes nothing about what the caller is told.
//!
//! Asynchronous also means NOBODY NEED CALL AGAIN: a client that publishes a datapoint or
//! writes an alarm looks for the notification on the action's target, not at CloudWatch, so the
//! transitions the clock makes due after that write are materialised — and the actions they
//! fire executed — by [`drive_alarm_clock`], the one bounded in-process task a write starts.
//! Every handler still settles what is due when it runs ([`settle_alarms`]); the clock is what
//! makes the path observable to a client that makes no further call.
//!
//! Only an SNS topic ARN is delivered to here. Every other action kind an alarm may carry (a
//! Lambda function, an Auto Scaling policy, an SSM OpsItem, an EC2 instance action) needs its
//! own invocation path, which is not implemented in this emulator: such an action is logged
//! and NOTHING is invented for it — no history item claiming it ran.

use ::serde_json::{Map, Value};

use crate::ops::types::{AlarmHistoryItem, CompositeAlarm, Dimension, MetricAlarm};
/// The ARN namespace of Amazon SNS, whose topics are the action kind this module delivers to.
const SNS_ARN_NAMESPACE: &str = "sns";

/// The most characters an SNS `Subject` may carry: "Subjects must be ASCII text that begins
/// with a letter, number, or punctuation mark; must not include line breaks or control
/// characters; and must be less than 100 characters long." (`PublishInput$Subject`)
const MAX_SUBJECT_CHARS: usize = 100;

/// How many owed notifications ONE request delivers.
///
/// The request's OWN bound, not a limit AWS publishes: every factor of the fan-out below it is
/// bounded by the model (five actions per state, a bounded document per notification), but
/// their PRODUCT with the number of queued notifications is not, and each action is an
/// in-process `Publish` that fans out again inside SNS. It is set far above any realistic
/// number of transitions one call makes due (a handful of alarms), and what it does not reach
/// is not dropped: it stays queued for the next call that settles.
const MAX_NOTIFICATIONS_PER_REQUEST: usize = 32;

/// The schema version the notification document declares, so a consumer can tell which
/// layout it is reading. The same version the alarm history documents declare.
const NOTIFICATION_VERSION: &str = super::HISTORY_DATA_VERSION;

/// One notification an alarm state transition owes: the document, and the action ARNs the
/// transition fires it at.
#[derive(Debug, Clone)]
pub struct AlarmNotification {
    /// The alarm whose transition owes this notification; its history records the delivery.
    pub alarm_name: String,
    /// Which kind of alarm that is, one of the modelled `AlarmType` values: what the history
    /// item recording the delivery is reported under.
    pub alarm_type: String,
    /// The state the alarm moved to, which is what selected [`Self::actions`].
    pub new_state_value: String,
    /// The action ARNs configured for the state the alarm moved to.
    pub actions: Vec<String>,
    /// The SNS `Subject` of the notification, when the alarm's name leaves room for one
    /// inside [`MAX_SUBJECT_CHARS`].
    pub subject: Option<String>,
    /// The notification document, as published.
    pub message: String,
}

impl AlarmNotification {
    /// The notification a METRIC alarm's transition to its current state owes, or `None` when
    /// the transition fires nothing: the alarm's actions are disabled, or it carries no action
    /// for the state it moved to.
    #[must_use]
    pub fn of_transition(
        alarm: &MetricAlarm,
        old_state_value: Option<&str>,
        at_secs: f64,
    ) -> Option<Self> {
        Transition {
            name: alarm.alarm_name.as_deref()?,
            kind: super::ALARM_TYPE_METRIC,
            arn: alarm.alarm_arn.as_deref(),
            description: alarm.alarm_description.as_deref(),
            actions_enabled: alarm.actions_enabled.unwrap_or(true),
            ok_actions: alarm.ok_actions.as_deref(),
            alarm_actions: alarm.alarm_actions.as_deref(),
            insufficient_data_actions: alarm.insufficient_data_actions.as_deref(),
            configuration_updated_secs: alarm
                .alarm_configuration_updated_timestamp
                .as_ref()
                .map(super::from_timestamp),
            new_state_value: alarm.state_value.as_deref()?,
            new_state_reason: alarm.state_reason.as_deref(),
            old_state_value,
            at_secs,
            detail: vec![("Trigger", metric_trigger_document(alarm))],
        }
        .build()
    }

    /// The notification a COMPOSITE alarm's transition to its current state owes. Same rule,
    /// same envelope: the alarm's own three action lists, selected by the state it moved to.
    /// A composite alarm reports the rule it evaluated, and the CHILDREN whose states that rule
    /// read, where a metric alarm reports the metric it evaluated.
    #[must_use]
    pub fn of_composite_transition(
        alarm: &CompositeAlarm,
        children: &[super::ChildAlarmState],
        old_state_value: Option<&str>,
        at_secs: f64,
    ) -> Option<Self> {
        // "When the value is ALARM, it means that the actions are suppressed because the
        // suppressor alarm is in ALARM" (`CompositeAlarm$ActionsSuppressedBy`) — a suppressed
        // transition fires nothing.
        if alarm.actions_suppressed_by.is_some() {
            return None;
        }
        Transition {
            name: alarm.alarm_name.as_deref()?,
            kind: super::ALARM_TYPE_COMPOSITE,
            arn: alarm.alarm_arn.as_deref(),
            description: alarm.alarm_description.as_deref(),
            actions_enabled: alarm.actions_enabled.unwrap_or(true),
            ok_actions: alarm.ok_actions.as_deref(),
            alarm_actions: alarm.alarm_actions.as_deref(),
            insufficient_data_actions: alarm.insufficient_data_actions.as_deref(),
            configuration_updated_secs: alarm
                .alarm_configuration_updated_timestamp
                .as_ref()
                .map(super::from_timestamp),
            new_state_value: alarm.state_value.as_deref()?,
            new_state_reason: alarm.state_reason.as_deref(),
            old_state_value,
            at_secs,
            detail: vec![
                (
                    "AlarmRule",
                    alarm
                        .alarm_rule
                        .as_deref()
                        .map_or(Value::Null, Value::from),
                ),
                ("TriggeringChildren", triggering_children(children)),
            ],
        }
        .build()
    }
}

/// One alarm state transition, as a notification is built from it.
///
/// Both kinds of alarm carry the same identity, the same three action lists and the same
/// three state members, so one definition builds both envelopes and the two cannot drift.
struct Transition<'a> {
    name: &'a str,
    kind: &'static str,
    arn: Option<&'a str>,
    description: Option<&'a str>,
    actions_enabled: bool,
    ok_actions: Option<&'a [String]>,
    alarm_actions: Option<&'a [String]>,
    insufficient_data_actions: Option<&'a [String]>,
    configuration_updated_secs: Option<f64>,
    new_state_value: &'a str,
    new_state_reason: Option<&'a str>,
    old_state_value: Option<&'a str>,
    at_secs: f64,
    /// The members that say WHAT was evaluated: a metric alarm's `Trigger`, a composite
    /// alarm's `AlarmRule` and the children its rule read. A member whose value is null is
    /// left out of the document.
    detail: Vec<(&'static str, Value)>,
}

impl Transition<'_> {
    /// The notification this transition owes, or `None` when it fires nothing.
    fn build(self) -> Option<AlarmNotification> {
        if !self.actions_enabled {
            return None;
        }
        let actions: Vec<String> = match self.new_state_value {
            super::ALARM_STATE_ALARM => self.alarm_actions,
            super::ALARM_STATE_OK => self.ok_actions,
            super::ALARM_STATE_INSUFFICIENT_DATA => self.insufficient_data_actions,
            _ => None,
        }
        .unwrap_or_default()
        .to_vec();
        if actions.is_empty() {
            return None;
        }

        // The Region and account the alarm belongs to, read off its own ARN rather than
        // guessed: the alarm was created in one Region and its notification reports that one.
        let parsed = self.arn.and_then(::service_common::arn::parse);
        let region = parsed.map(|arn| arn.region).unwrap_or_default();

        let mut document = Map::new();
        document.insert("Version".to_string(), Value::from(NOTIFICATION_VERSION));
        document.insert("AlarmName".to_string(), Value::from(self.name));
        insert(
            &mut document,
            "AlarmDescription",
            self.description.map(Value::from),
        );
        insert(
            &mut document,
            "AWSAccountId",
            parsed.map(|arn| Value::from(arn.account)),
        );
        insert(&mut document, "AlarmArn", self.arn.map(Value::from));
        insert(
            &mut document,
            "AlarmConfigurationUpdatedTimestamp",
            self.configuration_updated_secs
                .and_then(super::rfc3339)
                .map(Value::from),
        );
        document.insert(
            "NewStateValue".to_string(),
            Value::from(self.new_state_value),
        );
        insert(
            &mut document,
            "NewStateReason",
            self.new_state_reason.map(Value::from),
        );
        insert(
            &mut document,
            "OldStateValue",
            self.old_state_value.map(Value::from),
        );
        insert(
            &mut document,
            "StateChangeTime",
            super::rfc3339(self.at_secs).map(Value::from),
        );
        if !region.is_empty() {
            document.insert("Region".to_string(), Value::from(region));
        }
        document.insert(
            "OKActions".to_string(),
            string_list(self.ok_actions),
        );
        document.insert(
            "AlarmActions".to_string(),
            string_list(self.alarm_actions),
        );
        document.insert(
            "InsufficientDataActions".to_string(),
            string_list(self.insufficient_data_actions),
        );
        for (member, detail) in self.detail {
            if !detail.is_null() {
                document.insert(member.to_string(), detail);
            }
        }

        // "ALARM: "my-alarm" in us-east-1" — the subject SNS carries to an email subscriber.
        // It is omitted rather than truncated when the alarm's name does not leave room for
        // one inside the member's documented bound, because a truncated name names no alarm.
        let subject = format!("{}: \"{}\" in {region}", self.new_state_value, self.name);
        let subject = (subject.chars().count() < MAX_SUBJECT_CHARS).then_some(subject);

        Some(AlarmNotification {
            alarm_name: self.name.to_string(),
            alarm_type: self.kind.to_string(),
            new_state_value: self.new_state_value.to_string(),
            actions,
            subject,
            message: Value::Object(document).to_string(),
        })
    }
}

/// Materialise every alarm evaluation the clock has already made due, then DELIVER the action
/// notifications the resulting transitions owe.
///
/// The one entry point every handler that touches alarms calls: it holds the store's lock
/// only to evaluate and to take the queue, so the deliveries happen with the lock released
/// and one request's fan-out cannot block another's read. Delivery is at-most-once: a taken
/// notification is gone from the queue, so no later call can publish it twice.
///
/// ONE request's fan-out is bounded by [`MAX_NOTIFICATIONS_PER_REQUEST`]: that many
/// notifications are taken, each firing at most the five action ARNs a state's action list
/// may hold (`ResourceList` is modelled `@length 0..5`, its members `@length 1..1024`) and
/// each carrying one bounded document (`StateReason` `@length 0..1023`, `StateReasonData`
/// `@length 0..4000`), so the PRODUCT of the fan-out is capped and not just each factor of
/// it. What the budget does not reach stays queued, within
/// [`super::alarm_evaluation::MAX_PENDING_NOTIFICATIONS`], for the next call that settles —
/// which is what AWS's own asynchronous delivery looks like to a caller.
pub async fn settle_alarms<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
) {
    settle(engine, ctx.callback()).await;
}

/// The settle itself, over the handle it reaches the actions through: a request's own callback
/// ([`settle_alarms`]) or the one the alarm clock ([`drive_alarm_clock`]) carries, which is the
/// same handle into this same emulator.
async fn settle<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
) {
    let Some(state) = super::service_state(engine) else {
        // No store to evaluate. The handler that called this answers the store's absence with
        // its own modelled server fault; there is nothing to report from here.
        return;
    };
    let now = super::now_secs();
    let owed = {
        // The evaluation pass is CPU-bound and runs under the store's lock, so it is moved
        // off the async worker: the runtime keeps serving every other request while it runs,
        // and this request's own deadline has an await point to land on when it returns. The
        // work it may do is bounded per request by the budget below, so what the budget does
        // not reach stays due for the next call.
        crate::service::off_the_async_worker(|| {
            let mut budget = super::alarm_evaluation::EvaluationBudget::for_one_request();
            let mut store = state.lock();
            store.evaluate_metric_alarms(now, &mut budget);
            // A metric alarm's new state is an input to every composite alarm whose rule
            // references it, so the composite alarms are evaluated after it, in the same pass
            // and out of the same budget.
            store.evaluate_composite_alarms(now, &mut budget);
            store.take_pending_notifications(MAX_NOTIFICATIONS_PER_REQUEST)
        })
    };
    if owed.is_empty() {
        return;
    }

    let mut executed: Vec<(String, AlarmHistoryItem)> = Vec::new();
    let mut client: Option<(String, ::aws_sdk_sns::Client)> = None;
    for notification in owed {
        for action in &notification.actions {
            let Some(region) = ::service_common::arn::region_of(action, SNS_ARN_NAMESPACE) else {
                ::tracing::warn!(
                    alarm = %notification.alarm_name,
                    action = %action,
                    "this alarm action is not an SNS topic in a Region this emulator can \
                     reach; no action kind other than an SNS topic is executed here"
                );
                continue;
            };
            // One client per Region: every topic of one Region is reached through the same
            // endpoint, and a client built for another Region would sign for — and address
            // — the wrong one.
            let client = match client.as_ref() {
                Some((cached, client)) if cached == region => client,
                _ => {
                    let fresh = sns_client(callback, region);
                    &client.insert((region.to_string(), fresh)).1
                }
            };
            let published = client
                .publish()
                .topic_arn(action)
                .set_subject(notification.subject.clone())
                .message(&notification.message)
                .send()
                .await;
            let failure = published.err().map(|error| action_failure(&error));
            if let Some(failure) = failure.as_deref() {
                ::tracing::warn!(
                    alarm = %notification.alarm_name,
                    action = %action,
                    error = %failure,
                    "an alarm action could not be executed"
                );
            }
            executed.push((
                notification.alarm_name.clone(),
                action_history_item(&notification, action, failure.as_deref(), now),
            ));
        }
    }

    if executed.is_empty() {
        return;
    }
    // "Action" is one of the modelled `HistoryItemType` values: the record that the alarm
    // executed an action, which is where a caller sees that the notification went out.
    let mut store = state.lock();
    for (alarm_name, item) in executed {
        store.record_history(&alarm_name, now, item);
    }
}

/// Start the one in-process task that waits for this store's earliest due alarm evaluation,
/// materialises what is then due and delivers the action notifications those transitions owe,
/// and stands down once the store holds no alarm at all or the wake-up cap is reached.
///
/// A client that publishes a datapoint or writes an alarm makes no further CloudWatch call
/// before it looks for the notification on the action's target, so a transition the CLOCK makes
/// due — a datapoint ageing out of the evaluation range, the first evaluation after a write —
/// and the delivery it owes would never happen at all if only the next call could perform
/// them. This is what makes the action path observable without one. It does not change WHEN a
/// transition happens: the task only calls the same settle every handler calls, which applies
/// what the stored due instants have already made due, so a caller reading the alarm before
/// that instant still sees the state the write left behind. The task waits for the clock so
/// that no request ever has to.
///
/// At most one clock runs per store (the store carries the claim), it is bounded by
/// [`MAX_ALARM_WAKE_UPS`] wake-ups, and with no async runtime to spawn onto there is no clock
/// — the evaluations are then materialised by the next operation instead, as they always were.
pub fn drive_alarm_clock<Eng: Send + Sync + 'static>(
    engine: &::std::sync::Arc<Eng>,
    ctx: &smithy_server_runtime::OperationContext<'_>,
) {
    let Some(state) = super::service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = claim_alarm_clock(&mut state.lock()) else {
        return;
    };
    let engine = ::std::sync::Arc::clone(engine);
    // The same handle into this same emulator the request itself would have published
    // through, owned by the task rather than borrowed from the request that started it.
    let callback = ctx.callback().clone();
    let claimed = ::engine::ServiceState::clone(&state);
    let handle = runtime.spawn(async move {
        // `true` only if the wake-up cap is what ended the loop; a clock that ran out of work
        // has already released its claim inside the one critical section that observed it.
        let mut capped = true;
        for _ in 0..MAX_ALARM_WAKE_UPS {
            let Some(due_secs) = next_due_or_release(&mut state.lock(), generation) else {
                capped = false;
                break;
            };
            // Every wake-up waits, even for an instant already past: that is what keeps a due
            // evaluation from costing a CPU-bound spin, and it hands the runtime back between
            // wake-ups.
            let wait = ::std::time::Duration::from_secs_f64(
                (due_secs - super::now_secs()).clamp(0.0, MAX_ALARM_WAIT.as_secs_f64()),
            )
            .max(MIN_ALARM_WAIT);
            ::tokio::time::sleep(wait).await;
            settle(&*engine, &callback).await;
        }
        if capped {
            // Only THIS clock's claim, so a clock started since is left holding its own.
            release_alarm_clock(&mut state.lock(), generation);
        }
    });
    set_alarm_clock(&mut claimed.lock(), generation, handle);
}

/// The shortest an alarm-clock wake-up waits, so an evaluation already past its due instant
/// still costs a yield rather than a CPU-bound spin.
const MIN_ALARM_WAIT: ::std::time::Duration = ::std::time::Duration::from_millis(50);

/// The longest an alarm-clock wake-up waits, so the clock re-reads the store — which a write
/// may have changed since — at least this often.
const MAX_ALARM_WAIT: ::std::time::Duration = ::std::time::Duration::from_millis(500);

/// How many times one alarm clock wakes up before it stands down.
///
/// A backstop, not a schedule: it bounds the work one request can start, and whatever the
/// clock has not reached when it is spent is materialised by the next clock (every write
/// starts one if none is running) or by the next call that settles.
const MAX_ALARM_WAKE_UPS: usize = 2_000;

/// Claim the single alarm clock for this store: the claim's GENERATION when the caller may
/// start one, `None` when one already runs.
fn claim_alarm_clock(
    store: &mut super::ComAmazonawsCloudwatchGraniteserviceversion20100801State,
) -> Option<u64> {
    if store.alarm_clock_running {
        return None;
    }
    store.alarm_clock_running = true;
    store.alarm_clock_generation = store.alarm_clock_generation.wrapping_add(1);
    Some(store.alarm_clock_generation)
}

/// Release the claim `generation` took — and ONLY that one, so a clock that ended on the
/// wake-up cap cannot clear a claim a clock started since is holding.
fn release_alarm_clock(
    store: &mut super::ComAmazonawsCloudwatchGraniteserviceversion20100801State,
    generation: u64,
) {
    if store.alarm_clock_generation == generation {
        store.alarm_clock_running = false;
    }
}

/// Keep the running clock's task handle, aborting the one it replaces — and only while
/// `generation` is still the CURRENT claim, so a starter whose own task has already stood down
/// installs nothing and aborts nothing.
fn set_alarm_clock(
    store: &mut super::ComAmazonawsCloudwatchGraniteserviceversion20100801State,
    generation: u64,
    handle: ::tokio::task::JoinHandle<()>,
) {
    if store.alarm_clock_generation != generation {
        handle.abort();
        return;
    }
    if let Some(previous) = store.alarm_clock.replace(handle) {
        previous.abort();
    }
}

/// The instant the alarm clock waits for next, or `None` — having RELEASED its claim — when the
/// store owes nothing at all.
///
/// Observing "nothing is owed" and standing down happen in ONE critical section, which is what
/// makes the hand-over race-free: a write holds this same lock to store the alarm BEFORE it
/// tries to claim the clock, so either this call sees that alarm and the running clock keeps
/// it, or the write's claim finds the flag cleared and starts a clock of its own.
fn next_due_or_release(
    store: &mut super::ComAmazonawsCloudwatchGraniteserviceversion20100801State,
    generation: u64,
) -> Option<f64> {
    let due = store
        .alarms
        .values()
        .map(|stored| stored.evaluation_due_secs)
        .chain(
            store
                .composite_alarms
                .values()
                .map(|stored| stored.evaluation_due_secs),
        )
        .min_by(f64::total_cmp);
    if due.is_none() {
        release_alarm_clock(store, generation);
    }
    due
}

/// Why ONE action execution failed, as the alarm's history records it.
///
/// The TARGET's own answer: an action the invoked service refused failed for the reason that
/// service gave, so what is recorded is the `message` of the service error it answered with —
/// not a rendering of the client-side error chain that carried it, which names the types of
/// this emulator's transport rather than anything about the action. A failure the target never
/// answered at all (nothing dispatched, no response to parse) carries no such message, and for
/// it the chain is the only account of the failure there is.
fn action_failure<Response>(
    error: &::aws_smithy_runtime_api::client::result::SdkError<
        ::aws_sdk_sns::operation::publish::PublishError,
        Response,
    >,
) -> String
where
    Response: ::std::fmt::Debug,
{
    error
        .as_service_error()
        .and_then(::aws_smithy_types::error::metadata::ProvideErrorMetadata::message)
        .map_or_else(
            || ::aws_smithy_types::error::display::DisplayErrorContext(error).to_string(),
            ToString::to_string,
        )
}

/// The history item recording ONE action execution: what was executed, for which state, and
/// whether it succeeded.
fn action_history_item(
    notification: &AlarmNotification,
    action: &str,
    failure: Option<&str>,
    now_secs: f64,
) -> AlarmHistoryItem {
    let summary = match failure {
        None => format!("Successfully executed action {action}"),
        Some(_) => format!("Failed to execute action {action}"),
    };
    let mut document = Map::new();
    document.insert("version".to_string(), Value::from(NOTIFICATION_VERSION));
    document.insert(
        "actionState".to_string(),
        Value::from(if failure.is_some() {
            "Failed"
        } else {
            "Succeeded"
        }),
    );
    document.insert("notificationResource".to_string(), Value::from(action));
    document.insert(
        "stateValue".to_string(),
        Value::from(notification.new_state_value.as_str()),
    );
    // The member is present whether or not the action failed — `null` for one that succeeded —
    // because a reader of the record asks whether there WAS an error, and a member that is
    // simply absent answers that question with nothing at all.
    document.insert(
        "error".to_string(),
        failure.map_or(Value::Null, Value::from),
    );
    // What was actually published, so a reader of the record can tell the notification THIS
    // alarm's transition produced from any other message the target received. An action that
    // FAILED handed nothing over, so it reports no published message at all: nothing received
    // one.
    //
    // `AlarmHistoryItem$HistoryData` is `@length 1..4095` and the document is JSON, so it can
    // never be truncated to fit: the published message is the one part of it whose size the
    // alarm's own configuration drives, so it is the part dropped when the whole document
    // would not fit. Each form is rendered at most once, because the document carries the
    // notification itself and is the largest thing this item holds.
    let with_message = failure
        .is_none()
        .then(|| {
            let mut full = document.clone();
            full.insert(
                "publishedMessage".to_string(),
                Value::from(notification.message.as_str()),
            );
            Value::Object(full).to_string()
        })
        .filter(|rendered| rendered.len() <= super::MAX_HISTORY_DATA_LEN);
    let history_data =
        with_message.unwrap_or_else(|| Value::Object(document).to_string());
    AlarmHistoryItem {
        alarm_name: Some(notification.alarm_name.clone()),
        alarm_contributor_id: None,
        // The item belongs to the kind of alarm that executed the action, so a reader that
        // asks for one kind of alarm's history sees it.
        alarm_type: Some(notification.alarm_type.clone()),
        timestamp: Some(super::to_timestamp(now_secs)),
        history_item_type: Some("Action".to_string()),
        history_summary: Some(bounded(summary)),
        history_data: Some(history_data),
        alarm_contributor_attributes: None,
    }
}

/// An SNS client over the in-process callback into this same emulator.
fn sns_client(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_sns::Client {
    let config = ::aws_sdk_sns::Config::builder()
        .behavior_version(::aws_sdk_sns::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_sns::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://sns.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The same goes for caching the identity: the credentials are handed over
        // directly.
        .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_sns::Client::from_conf(config)
}

/// The children a composite alarm's transition reports: the alarms its rule read a state from,
/// each named by the ARN `DescribeAlarms` reports for it and carrying the state it is in and
/// when it moved there.
///
/// A consumer of the notification reads which child put the alarm where it is out of this,
/// which is why it is part of the envelope rather than only of the alarm's own explanation. A
/// child the rule read no state for is left out: it contributed nothing to the outcome.
fn triggering_children(children: &[super::ChildAlarmState]) -> Value {
    Value::Array(
        children
            .iter()
            .filter(|child| child.state_value.is_some())
            .map(|child| {
                let mut state = Map::new();
                insert(
                    &mut state,
                    "Value",
                    child.state_value.as_deref().map(Value::from),
                );
                insert(
                    &mut state,
                    "Timestamp",
                    child
                        .transitioned_secs
                        .and_then(super::rfc3339)
                        .map(Value::from),
                );
                let mut entry = Map::new();
                entry.insert("Arn".to_string(), Value::from(child.identity()));
                entry.insert("State".to_string(), Value::Object(state));
                Value::Object(entry)
            })
            .collect(),
    )
}

/// A metric alarm's `Trigger`: the configuration the evaluation applied, so a consumer of the
/// notification can see which metric crossed which threshold.
fn metric_trigger_document(alarm: &MetricAlarm) -> Value {
    let mut trigger = Map::new();
    insert(
        &mut trigger,
        "MetricName",
        alarm.metric_name.as_deref().map(Value::from),
    );
    insert(
        &mut trigger,
        "Namespace",
        alarm.namespace.as_deref().map(Value::from),
    );
    // "StatisticType" says which of the two statistic members the alarm carries, the way the
    // model names them (`MetricAlarm$Statistic` / `MetricAlarm$ExtendedStatistic`).
    if let Some(statistic) = alarm.statistic.as_deref() {
        trigger.insert("StatisticType".to_string(), Value::from("Statistic"));
        trigger.insert("Statistic".to_string(), Value::from(statistic));
    } else if let Some(extended) = alarm.extended_statistic.as_deref() {
        trigger.insert(
            "StatisticType".to_string(),
            Value::from("ExtendedStatistic"),
        );
        trigger.insert("ExtendedStatistic".to_string(), Value::from(extended));
    }
    insert(&mut trigger, "Unit", alarm.unit.as_deref().map(Value::from));
    insert(
        &mut trigger,
        "Dimensions",
        alarm.dimensions.as_deref().map(dimensions_document),
    );
    insert(&mut trigger, "Period", alarm.period.map(Value::from));
    insert(
        &mut trigger,
        "EvaluationPeriods",
        alarm.evaluation_periods.map(Value::from),
    );
    insert(
        &mut trigger,
        "DatapointsToAlarm",
        alarm.datapoints_to_alarm.map(Value::from),
    );
    insert(
        &mut trigger,
        "ComparisonOperator",
        alarm.comparison_operator.as_deref().map(Value::from),
    );
    insert(&mut trigger, "Threshold", alarm.threshold.map(Value::from));
    insert(
        &mut trigger,
        "TreatMissingData",
        alarm.treat_missing_data.as_deref().map(Value::from),
    );
    insert(
        &mut trigger,
        "EvaluateLowSampleCountPercentile",
        alarm
            .evaluate_low_sample_count_percentile
            .as_deref()
            .map(Value::from),
    );
    Value::Object(trigger)
}

/// A dimension list as the notification's `Trigger` carries it.
fn dimensions_document(dimensions: &[Dimension]) -> Value {
    Value::Array(
        dimensions
            .iter()
            .map(|dimension| {
                Value::Object(
                    [
                        ("name".to_string(), Value::from(dimension.name.as_str())),
                        ("value".to_string(), Value::from(dimension.value.as_str())),
                    ]
                    .into_iter()
                    .collect(),
                )
            })
            .collect(),
    )
}

/// An action list as the notification carries it: the ARNs the alarm holds, and an empty array
/// when it holds none.
fn string_list(values: Option<&[String]>) -> Value {
    Value::Array(
        values
            .unwrap_or_default()
            .iter()
            .map(|value| Value::from(value.as_str()))
            .collect(),
    )
}

/// Add `value` to a document under `member`, and nothing at all when there is no value.
fn insert(document: &mut Map<String, Value>, member: &str, value: Option<Value>) {
    if let Some(value) = value {
        document.insert(member.to_string(), value);
    }
}

/// One history summary, inside the modelled `@length 1..255` of
/// `AlarmHistoryItem$HistorySummary`.
///
/// An action ARN is `@length 1..1024`, so a summary naming one need not fit: the ARN is the
/// variable part, and a summary that does not fit is cut at the bound rather than dropped,
/// because the item's own `HistoryData` carries the whole ARN.
fn bounded(summary: String) -> String {
    /// The modelled maximum length of `AlarmHistoryItem$HistorySummary`.
    const MAX_HISTORY_SUMMARY_LEN: usize = 255;

    if summary.len() <= MAX_HISTORY_SUMMARY_LEN {
        return summary;
    }
    summary
        .char_indices()
        .take_while(|(index, character)| index + character.len_utf8() <= MAX_HISTORY_SUMMARY_LEN)
        .map(|(_, character)| character)
        .collect()
}
