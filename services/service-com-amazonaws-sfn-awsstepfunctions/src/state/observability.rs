// SPDX-License-Identifier: CC0-1.0

//! What an execution PUBLISHES about itself: the `AWS/States` metric series every
//! execution is counted in, and the history events a state machine's
//! `loggingConfiguration` delivers to CloudWatch Logs.
//!
//! An execution runs with no client action at all once it has been started, so the
//! only way a client can see that it started, how long it ran and whether it succeeded
//! — and the only way an alarm, a dashboard or a log-based assertion over Step
//! Functions can see anything — is what the service publishes about its own work:
//!
//! * the METRICS go to the [`METRIC_NAMESPACE`] namespace, dimensioned on the state
//!   machine the execution belongs to ([`STATE_MACHINE_ARN_DIMENSION`]): one datapoint
//!   per execution started, one per execution that reached a terminal status under the
//!   metric for that status, and the span it ran for. An EXPRESS workflow is billed
//!   per run, so it also publishes the two billed series, whose values are the very
//!   figures `StartSyncExecution` reports as `billingDetails` — summing a window of
//!   either series therefore gives the summed billing details of the runs in it, and a
//!   STANDARD workflow, which is not billed that way, publishes neither;
//! * the LOG EVENTS go to the log group the configuration names, with the events
//!   selected by its `level` ([`delivered_at_level`]) and the execution data included
//!   only where `includeExecutionData` asks for it.
//!
//! Both are written into the stores the OWNING service's own readers answer from —
//! CloudWatch's for the metrics, CloudWatch Logs' for the events — through those
//! services' own record types, so a published metric and `GetMetricData`, and a
//! delivered event and `FilterLogEvents`, cannot disagree. Neither goes out over
//! `PutMetricData` or `PutLogEvents`: CloudWatch reserves the `AWS/` namespaces for
//! the services that own them ("you should not specify a namespace that begins with
//! `AWS/`", `PutMetricData$Namespace`), so a service's own metric never travels over
//! that API.
//!
//! ## When it is published
//!
//! An observation is QUEUED on this service's own store at the instant the thing it
//! measures happens — an execution admitted, an outcome the clock made due — and
//! PUBLISHED by the call that queued it, once that call has released the store
//! ([`publish`]). The queue is what keeps the two stores from being locked at once and
//! what lets the work that produces an observation stay a pure store transition; it is
//! bounded ([`MAX_PENDING_OBSERVATIONS`]), so a deployment with no CloudWatch mounted
//! cannot grow it without end.

use ::std::collections::BTreeMap;
use ::std::time::{SystemTime, UNIX_EPOCH};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;
use ::service_com_amazonaws_cloudwatchlogs_logs_20140328::state as logs;

use super::ComAmazonawsSfnAwsstepfunctionsState;

/// The namespace Step Functions publishes its own metrics to.
pub const METRIC_NAMESPACE: &str = "AWS/States";

/// The dimension those series are reported per: the state machine the execution
/// belongs to, named by its ARN.
pub const STATE_MACHINE_ARN_DIMENSION: &str = "StateMachineArn";

/// The series counting the executions that were STARTED.
pub const EXECUTIONS_STARTED_METRIC: &str = "ExecutionsStarted";

/// The series counting the executions that completed successfully.
pub const EXECUTIONS_SUCCEEDED_METRIC: &str = "ExecutionsSucceeded";

/// The series counting the executions that failed.
pub const EXECUTIONS_FAILED_METRIC: &str = "ExecutionsFailed";

/// The series counting the executions that were aborted.
pub const EXECUTIONS_ABORTED_METRIC: &str = "ExecutionsAborted";

/// The series counting the executions that timed out.
pub const EXECUTIONS_TIMED_OUT_METRIC: &str = "ExecutionsTimedOut";

/// The series measuring how long an execution took, from start to terminal status.
pub const EXECUTION_TIME_METRIC: &str = "ExecutionTime";

/// The series measuring the duration an EXPRESS run is billed for, which is the
/// `billedDurationInMilliseconds` of its `billingDetails`.
pub const EXPRESS_BILLED_DURATION_METRIC: &str = "ExpressExecutionBilledDuration";

/// The series measuring the memory an EXPRESS run is billed for, which is the
/// `billedMemoryUsedInMB` of its `billingDetails`.
pub const EXPRESS_BILLED_MEMORY_METRIC: &str = "ExpressExecutionBilledMemory";

/// The `StandardUnit` a bare count of things carries.
pub const COUNT_UNIT: &str = "Count";

/// The `StandardUnit` a span of milliseconds carries.
pub const MILLISECONDS_UNIT: &str = "Milliseconds";

/// The `StandardUnit` an amount of memory in megabytes carries.
pub const MEGABYTES_UNIT: &str = "Megabytes";

/// The `StorageResolution` these datapoints are published at, in seconds: regular
/// resolution, "which CloudWatch stores at 1-minute resolution"
/// (`MetricDatum$StorageResolution`).
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// `LogLevel`'s value that logs every history event ("ALL – Log all events").
pub const LOG_LEVEL_ALL: &str = "ALL";

/// `LogLevel`'s value that logs the failures ("ERROR – Log all events of type ERROR
/// and FATAL").
pub const LOG_LEVEL_ERROR: &str = "ERROR";

/// `LogLevel`'s value that logs the execution-level failures alone ("FATAL – Log all
/// events of type FATAL").
pub const LOG_LEVEL_FATAL: &str = "FATAL";

/// How many observations this service's store queues before it drops them.
///
/// The queue is drained by the call that filled it, so it normally holds the few
/// observations of one call; the bound is what keeps a deployment with no CloudWatch
/// mounted — where nothing is ever published — from growing it with every execution.
pub const MAX_PENDING_OBSERVATIONS: usize = 4096;

/// ONE measurement an execution owes a series, waiting to be published.
#[derive(Debug, Clone)]
pub struct Datapoint {
    /// The state machine the series is dimensioned on.
    pub state_machine_arn: String,
    /// The metric the measurement belongs to.
    pub metric: &'static str,
    /// What was measured.
    pub value: f64,
    /// The unit it was measured in.
    pub unit: &'static str,
    /// The instant it was taken at.
    pub at: SystemTime,
}

/// The history events of ONE execution a logging configuration owes a log group.
#[derive(Debug, Clone)]
pub struct Delivery {
    /// The log group the configuration names, by the NAME CloudWatch Logs holds it
    /// under.
    pub log_group: String,
    /// The Region that group lives in, read off the ARN the configuration carried, so
    /// a group this delivery has to create carries the ARN its own reader reports.
    pub region: String,
    /// The stream the events are written to: one per execution.
    pub log_stream: String,
    /// Each event as `(timestamp in epoch milliseconds, message)`.
    pub events: Vec<(i64, String)>,
}

/// The observations this service's store has queued and not published yet.
#[derive(Debug, Default)]
pub struct Pending {
    /// The metric measurements.
    pub datapoints: Vec<Datapoint>,
    /// The log deliveries.
    pub deliveries: Vec<Delivery>,
}

impl Pending {
    /// Whether there is nothing to publish.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.datapoints.is_empty() && self.deliveries.is_empty()
    }

    /// Queue `datapoint`, or drop it when the queue is full (see
    /// [`MAX_PENDING_OBSERVATIONS`]).
    fn queue_datapoint(&mut self, datapoint: Datapoint) {
        if self.datapoints.len() >= MAX_PENDING_OBSERVATIONS {
            return;
        }
        self.datapoints.push(datapoint);
    }

    /// Queue `delivery`, or drop it when the queue is full.
    fn queue_delivery(&mut self, delivery: Delivery) {
        if self.deliveries.len() >= MAX_PENDING_OBSERVATIONS {
            return;
        }
        self.deliveries.push(delivery);
    }
}

/// `at` in milliseconds since the Unix epoch, which is what every CloudWatch Logs
/// timestamp is expressed in (`OutputLogEvent$timestamp`).
fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// `at` in seconds since the Unix epoch, which is how a CloudWatch datapoint carries
/// its timestamp.
fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// The metric an execution that reached `status` is counted under, or `None` for a
/// status that is not terminal: a running execution has not completed, so it is
/// counted in no completion series yet.
fn completion_metric(status: &str) -> Option<&'static str> {
    match status {
        super::EXECUTION_STATUS_SUCCEEDED => Some(EXECUTIONS_SUCCEEDED_METRIC),
        super::EXECUTION_STATUS_FAILED => Some(EXECUTIONS_FAILED_METRIC),
        super::EXECUTION_STATUS_ABORTED => Some(EXECUTIONS_ABORTED_METRIC),
        super::EXECUTION_STATUS_TIMED_OUT => Some(EXECUTIONS_TIMED_OUT_METRIC),
        _ => None,
    }
}

/// Whether an event of type `event_type` is delivered under the logging `level`.
///
/// `LogLevel`'s own documentation: "ALL – Log all events. ERROR – Log all events of
/// type ERROR and FATAL. FATAL – Log all events of type FATAL. OFF – No logging." So
/// the decision is which CLASS the event type belongs to:
///
/// * FATAL is the end of the execution itself, which is what a workflow that failed,
///   was aborted or timed out reports;
/// * ERROR is every other event reporting that something did not succeed — a task, an
///   activity, an iteration, a branch or a map run that failed, timed out or was
///   aborted, whose type names end the same way the execution-level ones do;
/// * everything else is the ordinary progress of the run, which only `ALL` logs.
#[must_use]
pub fn delivered_at_level(level: &str, event_type: &str) -> bool {
    let fatal = matches!(
        event_type,
        super::EVENT_EXECUTION_FAILED
            | super::EVENT_EXECUTION_ABORTED
            | super::EVENT_EXECUTION_TIMED_OUT
    );
    let error = fatal
        || event_type.ends_with("Failed")
        || event_type.ends_with("TimedOut")
        || event_type.ends_with("Aborted");
    match level {
        LOG_LEVEL_ALL => true,
        LOG_LEVEL_ERROR => error,
        LOG_LEVEL_FATAL => fatal,
        // `OFF`, and any value the enum does not name, deliver nothing.
        _ => false,
    }
}

/// The log group a `loggingConfiguration` delivers to, as `(region, name)`, or `None`
/// when it names none.
///
/// The ARN the configuration carries is the qualified log group ARN
/// `arn:<partition>:logs:<region>:<account>:log-group:<name>:*` — the form
/// `logGroupArn` documents and `crate::state::invalid_logging_reason` admits — so the
/// Region and the name CloudWatch Logs holds the group under are read out of it rather
/// than assumed.
#[must_use]
pub fn log_group_of(configuration: &crate::ops::types::LoggingConfiguration) -> Option<(String, String)> {
    let arn = configuration
        .destinations
        .as_deref()?
        .first()?
        .cloud_watch_logs_log_group
        .as_ref()?
        .log_group_arn
        .as_deref()?;
    let mut fields = arn.splitn(6, ':');
    let _scheme = fields.next()?;
    let _partition = fields.next()?;
    let _service = fields.next()?;
    let region = fields.next()?.to_string();
    let _account = fields.next()?;
    let resource = fields.next()?;
    let name = resource
        .strip_prefix("log-group:")?
        .strip_suffix(":*")?
        .to_string();
    (!name.is_empty()).then_some((region, name))
}

/// The stream one execution's events are delivered to.
///
/// Step Functions writes a stream per execution under the `states/` prefix every
/// vended Step Functions log stream carries, so two executions of one state machine
/// never interleave in a stream and a reader can tell which run an event belongs to
/// from the stream alone — while a reader of the GROUP (`FilterLogEvents`) sees them
/// all.
fn log_stream_of(state_machine_name: &str, execution_name: &str) -> String {
    format!("states/{state_machine_name}/{execution_name}")
}

/// One history event as a delivered log message.
///
/// The members are the event's own identity — the id, the type, the event it follows
/// and the instant it happened — beside the execution it belongs to, which is what
/// makes a delivered event traceable back to the history it came from. The execution
/// DATA an event carries (the input it was handed, the output it produced, the error
/// and cause of a failure) is included only when the configuration's
/// `includeExecutionData` asks for it: "determines whether execution data is included
/// in your log".
fn log_message(
    event: &super::HistoryEvent,
    execution_arn: &str,
    include_execution_data: bool,
) -> String {
    let mut message = ::serde_json::Map::new();
    message.insert("id".to_string(), event.id.to_string().into());
    message.insert("type".to_string(), event.event_type.clone().into());
    message.insert(
        "previous_event_id".to_string(),
        event.previous_event_id.to_string().into(),
    );
    message.insert(
        "event_timestamp".to_string(),
        epoch_millis(event.timestamp).to_string().into(),
    );
    message.insert("execution_arn".to_string(), execution_arn.to_string().into());
    if include_execution_data {
        let mut details = ::serde_json::Map::new();
        for (member, value) in [
            ("input", event.details.input.as_deref()),
            ("output", event.details.output.as_deref()),
            ("error", event.details.error.as_deref()),
            ("cause", event.details.cause.as_deref()),
        ] {
            if let Some(value) = value {
                details.insert(member.to_string(), value.to_string().into());
            }
        }
        if !details.is_empty() {
            message.insert("details".to_string(), ::serde_json::Value::Object(details));
        }
    }
    ::serde_json::Value::Object(message).to_string()
}

impl ComAmazonawsSfnAwsstepfunctionsState {
    /// Queue what the execution `execution_arn` publishes about having been STARTED,
    /// and — for a record that is already terminal, which a synchronous Express run
    /// is — everything it publishes about having ENDED.
    ///
    /// Called by the one place a started execution enters the store, so an execution
    /// nothing admitted publishes nothing.
    pub(super) fn observe_execution_started(&mut self, execution_arn: &str) {
        let Some(execution) = self.executions.get(execution_arn) else {
            return;
        };
        let datapoint = Datapoint {
            state_machine_arn: execution.state_machine_arn.clone(),
            metric: EXECUTIONS_STARTED_METRIC,
            value: 1.0,
            unit: COUNT_UNIT,
            at: execution.start_date,
        };
        let terminal = execution.is_terminal();
        self.pending_observations.queue_datapoint(datapoint);
        if terminal {
            self.observe_execution_settled(execution_arn);
        }
    }

    /// Queue what the execution `execution_arn` publishes about having reached a
    /// TERMINAL status: the series for that status, the span it ran for, the billed
    /// series of an Express run, and the history events its state machine's logging
    /// configuration delivers.
    ///
    /// Called by the one place an owed outcome is materialised (`settle`) and by
    /// [`Self::observe_execution_started`] for a record that arrives terminal, so
    /// every execution that ends is counted exactly once.
    pub(super) fn observe_execution_settled(&mut self, execution_arn: &str) {
        let Some(execution) = self.executions.get(execution_arn) else {
            return;
        };
        let Some(metric) = completion_metric(&execution.status) else {
            return;
        };
        let machine_arn = execution.state_machine_arn.clone();
        let stopped = execution.stop_date.unwrap_or(execution.start_date);
        let ran_millis = stopped
            .duration_since(execution.start_date)
            .map(|span| span.as_millis())
            .unwrap_or_default();
        let observation = |metric: &'static str, value: f64, unit: &'static str| Datapoint {
            state_machine_arn: machine_arn.clone(),
            metric,
            value,
            unit,
            at: stopped,
        };
        let mut queued = vec![
            observation(metric, 1.0, COUNT_UNIT),
            observation(
                EXECUTION_TIME_METRIC,
                ran_millis as f64,
                MILLISECONDS_UNIT,
            ),
        ];
        let machine = self.state_machines.get(&machine_arn);
        let express =
            machine.is_some_and(|machine| machine.sm_type == super::STATE_MACHINE_TYPE_EXPRESS);
        if express {
            // The figures `StartSyncExecution` reports as `billingDetails`, computed
            // from the same place it computes them, so a window summed over either
            // series equals the billing details of the runs in it.
            let billed_duration = super::billed_duration_millis(execution.start_date, stopped);
            let billed_memory = super::billed_memory_mb(execution.measure());
            queued.push(observation(
                EXPRESS_BILLED_DURATION_METRIC,
                billed_duration as f64,
                MILLISECONDS_UNIT,
            ));
            queued.push(observation(
                EXPRESS_BILLED_MEMORY_METRIC,
                billed_memory as f64,
                MEGABYTES_UNIT,
            ));
        }
        // The events the state machine's logging configuration delivers, selected by
        // its own `level` and carrying the execution data only where it asks for it.
        let delivery = machine.and_then(|machine| {
            let configuration = machine.logging_configuration.as_ref()?;
            let level = configuration.level.as_deref().unwrap_or(super::LOG_LEVEL_OFF);
            let (region, log_group) = log_group_of(configuration)?;
            let events: Vec<(i64, String)> = execution
                .history
                .iter()
                .filter(|event| delivered_at_level(level, &event.event_type))
                .map(|event| {
                    (
                        epoch_millis(event.timestamp),
                        log_message(event, &execution.arn, configuration.include_execution_data),
                    )
                })
                .collect();
            (!events.is_empty()).then(|| Delivery {
                log_group,
                region,
                log_stream: log_stream_of(&machine.name, &execution.name),
                events,
            })
        });
        for datapoint in queued {
            self.pending_observations.queue_datapoint(datapoint);
        }
        if let Some(delivery) = delivery {
            self.pending_observations.queue_delivery(delivery);
        }
    }

    /// Take the observations queued so far, leaving the queue empty: what [`publish`]
    /// writes into the owning services' stores once this one's guard is released.
    pub fn take_observations(&mut self) -> Pending {
        ::std::mem::take(&mut self.pending_observations)
    }
}

/// Publish everything this service's store has queued: the metric datapoints into
/// CloudWatch's own store, and the history events into CloudWatch Logs'.
///
/// Call it with NO store of this service locked — it takes this service's guard to
/// drain the queue, and then the owning service's to write — so the two are never held
/// at once. An engine carrying neither store has nowhere to publish to, so the
/// observations are dropped rather than kept for a reader that cannot exist.
pub fn publish<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let pending = {
        let mut store = state.lock();
        store.take_observations()
    };
    if pending.is_empty() {
        return;
    }
    publish_datapoints(engine, &pending.datapoints);
    deliver_events(engine, &pending.deliveries);
}

/// Append every datapoint to the CloudWatch store of the SAME engine — the store
/// `GetMetricData`, `GetMetricStatistics` and `ListMetrics` answer from — and make the
/// alarms watching those series due, exactly as an ingested `PutMetricData` datapoint
/// does, so an alarm cannot tell a service's own metric from a client's.
fn publish_datapoints<Eng: Send + Sync + 'static>(engine: &Eng, datapoints: &[Datapoint]) {
    if datapoints.is_empty() {
        return;
    }
    let Some(metrics) = cloudwatch::service_state(engine) else {
        return;
    };
    let mut store = metrics.lock();
    for datapoint in datapoints {
        let dimensions: cloudwatch::DimensionSet = [(
            STATE_MACHINE_ARN_DIMENSION.to_string(),
            datapoint.state_machine_arn.clone(),
        )]
        .into_iter()
        .collect();
        let key = cloudwatch::MetricKey {
            namespace: METRIC_NAMESPACE.to_string(),
            metric_name: datapoint.metric.to_string(),
            dimensions,
        };
        let at_secs = epoch_secs(datapoint.at);
        let datum = cloudwatch::StoredDatum {
            timestamp_secs: at_secs,
            sample_count: 1.0,
            sum: datapoint.value,
            minimum: datapoint.value,
            maximum: datapoint.value,
            unit: Some(datapoint.unit.to_string()),
            storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
        };
        let watched = cloudwatch::WrittenSeries {
            key: key.clone(),
            units: vec![datapoint.unit.to_string()],
        };
        // A store already holding its full complement of series drops the observation
        // rather than failing the work it is about: a metric is published off the
        // response path.
        if store.append_datapoints(key, vec![datum]).is_err() {
            ::tracing::warn!(
                metric = datapoint.metric,
                "the CloudWatch store is at capacity; a Step Functions metric was not published"
            );
            continue;
        }
        store.make_alarms_due(&[watched], at_secs);
    }
}

/// Write every delivery into the CloudWatch Logs store of the SAME engine — the store
/// `FilterLogEvents`, `GetLogEvents` and `DescribeLogStreams` answer from.
///
/// The log group a configuration names is CREATED if the account does not hold it yet:
/// the configuration says where the executions' logs go, and a group a reader cannot
/// find is a destination nothing reaches. A group the account ALREADY holds is left
/// exactly as it is — its retention, its tags and the events already in it belong to
/// the account — and so is a group this emulator has no room for, which takes no
/// delivery rather than displacing another one.
///
/// Every factor a caller can grow is admitted against the OWNING service's own
/// ceiling before anything is written: how many groups the store holds
/// ([`logs::MAX_LOG_GROUPS`]), how many streams one group holds
/// ([`logs::MAX_STREAMS_PER_GROUP`], the ceiling that service's own `CreateLogStream`
/// admits one against) and how much one stream retains (its own
/// `enforce_storage_ceiling`), so the product of the executions started and the events
/// each delivers is bounded by that store rather than by the requests made.
fn deliver_events<Eng: Send + Sync + 'static>(engine: &Eng, deliveries: &[Delivery]) {
    if deliveries.is_empty() {
        return;
    }
    let Some(state) = logs::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    for delivery in deliveries {
        let now_ms = logs::now_ms();
        if !store.log_groups.contains_key(&delivery.log_group)
            && store.log_groups.len() >= logs::MAX_LOG_GROUPS
        {
            ::tracing::warn!(
                log_group = %delivery.log_group,
                "the CloudWatch Logs store is at capacity; a Step Functions log delivery was \
                 not made"
            );
            continue;
        }
        let events: Vec<logs::StoredEvent> = delivery
            .events
            .iter()
            .map(|(timestamp, message)| logs::StoredEvent {
                timestamp: *timestamp,
                message: message.clone(),
                ingestion_time: now_ms,
                ingestion_seq: store.take_ingestion_seq(),
            })
            .collect();
        let added: i64 = events
            .iter()
            .map(logs::StoredEvent::stored_bytes)
            .fold(0i64, i64::saturating_add);
        let group = store
            .log_groups
            .entry(delivery.log_group.clone())
            .or_insert_with(|| {
                logs::LogGroupRecord::new(&delivery.region, &delivery.log_group, now_ms)
            });
        // A stream is opened per EXECUTION, so how many of them one group holds grows
        // with the executions a caller starts: the delivery is admitted against the
        // same per-group ceiling the owning service's own `CreateLogStream` admits one
        // against ([`logs::MAX_STREAMS_PER_GROUP`]), so no write path can take a group
        // past it. A group already holding its full complement takes no further
        // delivery rather than displacing a stream the account holds.
        if !group.streams.contains_key(&delivery.log_stream)
            && group.streams.len() >= logs::MAX_STREAMS_PER_GROUP
        {
            ::tracing::warn!(
                log_group = %delivery.log_group,
                log_stream = %delivery.log_stream,
                "the log group already holds the maximum number of streams; a Step Functions \
                 log delivery was not made"
            );
            continue;
        }
        let stream = group
            .streams
            .entry(delivery.log_stream.clone())
            .or_insert_with(|| {
                logs::LogStreamRecord::new(
                    &delivery.region,
                    &delivery.log_group,
                    &delivery.log_stream,
                    now_ms,
                )
            });
        stream.events.extend(events);
        // The stream is kept in `(timestamp, ingestion_seq)` order, which is the order
        // its reader pages it in; a run's events are already in that order, so only a
        // delivery that predates events already there needs the sort.
        stream.events.sort_by_key(logs::StoredEvent::key);
        let discarded = stream.enforce_storage_ceiling();
        stream.refresh_event_times();
        stream.last_ingestion_time = Some(now_ms);
        group.stored_bytes = group
            .stored_bytes
            .saturating_add(added)
            .saturating_sub(discarded)
            .max(0);
    }
}

/// The dimension set a series of this service is identified by, for a reader to ask
/// for the same series the publisher wrote.
///
/// `pub` so this crate's tests read the series through the same construction the
/// publisher writes them under.
#[must_use]
pub fn state_machine_dimensions(state_machine_arn: &str) -> BTreeMap<String, String> {
    [(
        STATE_MACHINE_ARN_DIMENSION.to_string(),
        state_machine_arn.to_string(),
    )]
    .into_iter()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a logging `level` selects the CLASS of history event delivered — `ALL`
    /// every event, `ERROR` the failures, `FATAL` the execution-level ones alone, and
    /// `OFF` none.
    ///
    /// The rows span the three classes of event type (ordinary progress, a failure of
    /// one state's work, the end of the execution) against every value of the enum.
    #[test]
    fn a_level_selects_the_class_of_event_it_delivers() {
        // (event type, delivered at ALL, at ERROR, at FATAL, at OFF)
        let rows = [
            (super::super::EVENT_EXECUTION_STARTED, true, false, false),
            (super::super::EVENT_EXECUTION_SUCCEEDED, true, false, false),
            ("TaskStateEntered", true, false, false),
            ("TaskFailed", true, true, false),
            ("ActivityTimedOut", true, true, false),
            ("MapRunAborted", true, true, false),
            (super::super::EVENT_EXECUTION_FAILED, true, true, true),
            (super::super::EVENT_EXECUTION_ABORTED, true, true, true),
            (super::super::EVENT_EXECUTION_TIMED_OUT, true, true, true),
        ];
        for (event_type, at_all, at_error, at_fatal) in rows {
            for (level, expected) in [
                (LOG_LEVEL_ALL, at_all),
                (LOG_LEVEL_ERROR, at_error),
                (LOG_LEVEL_FATAL, at_fatal),
                (super::super::LOG_LEVEL_OFF, false),
            ] {
                let delivered = delivered_at_level(level, event_type);
                assert_eq!(
                    delivered, expected,
                    "case {event_type:?} at level {level:?}: expected delivered={expected}, got \
                     {delivered}"
                );
            }
        }
    }

    /// RULE: the log group a configuration delivers to is the one its qualified
    /// `logGroupArn` names, in that ARN's own Region; a value that is not such an ARN
    /// names no group.
    #[test]
    fn the_destination_arn_names_the_group_and_its_region() {
        let destination = |arn: &str| crate::ops::types::LoggingConfiguration {
            level: Some(LOG_LEVEL_ALL.to_string()),
            include_execution_data: true,
            destinations: Some(vec![crate::ops::types::LogDestination {
                cloud_watch_logs_log_group: Some(crate::ops::types::CloudWatchLogsLogGroup {
                    log_group_arn: Some(arn.to_string()),
                }),
            }]),
        };
        // One row: the ARN a configuration carries, and the group it names.
        struct Row {
            case: &'static str,
            arn: &'static str,
            /// The Region and the name the ARN names, and `None` for a value that
            /// names no log group at all.
            names: Option<(&'static str, &'static str)>,
        }
        let rows = [
            Row {
                case: "a qualified log group ARN",
                arn: "arn:aws:logs:us-east-1:000000000000:log-group:/aws/vendedlogs/states/one:*",
                names: Some(("us-east-1", "/aws/vendedlogs/states/one")),
            },
            Row {
                case: "another Region's group",
                arn: "arn:aws:logs:eu-west-2:000000000000:log-group:sfn-logs:*",
                names: Some(("eu-west-2", "sfn-logs")),
            },
            Row {
                case: "an ARN with no qualifier",
                arn: "arn:aws:logs:us-east-1:000000000000:log-group:sfn-logs",
                names: None,
            },
            Row {
                case: "an ARN of another resource type",
                arn: "arn:aws:logs:us-east-1:000000000000:destination:sfn-logs:*",
                names: None,
            },
            Row {
                case: "a value that is no ARN",
                arn: "sfn-logs",
                names: None,
            },
        ];
        for Row { case, arn, names } in rows {
            let read = log_group_of(&destination(arn));
            let expected = names.map(|(region, name)| (region.to_string(), name.to_string()));
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: an execution is counted in the completion series of the status it reached,
    /// and a status that is not terminal is counted in none.
    #[test]
    fn a_terminal_status_selects_its_completion_series() {
        for (status, expected) in [
            (super::super::EXECUTION_STATUS_SUCCEEDED, Some(EXECUTIONS_SUCCEEDED_METRIC)),
            (super::super::EXECUTION_STATUS_FAILED, Some(EXECUTIONS_FAILED_METRIC)),
            (super::super::EXECUTION_STATUS_ABORTED, Some(EXECUTIONS_ABORTED_METRIC)),
            (super::super::EXECUTION_STATUS_TIMED_OUT, Some(EXECUTIONS_TIMED_OUT_METRIC)),
            (super::super::EXECUTION_STATUS_RUNNING, None),
            ("PENDING_REDRIVE", None),
        ] {
            let metric = completion_metric(status);
            assert_eq!(
                metric, expected,
                "case {status:?}: expected {expected:?}, got {metric:?}"
            );
        }
    }
}
