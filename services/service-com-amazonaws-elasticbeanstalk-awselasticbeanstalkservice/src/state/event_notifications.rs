// SPDX-License-Identifier: CC0-1.0

//! The notifications an environment's events are DELIVERED as: the Amazon SNS
//! topic an environment's configuration names, the topic Elastic Beanstalk
//! creates for an environment whose configuration names only an endpoint, and
//! the publish each recorded event owes that topic.
//!
//! Elastic Beanstalk's published option reference puts those settings in the
//! `aws:elasticbeanstalk:sns:topics` namespace — the endpoint "where you want to
//! be notified of important events affecting your application", the protocol
//! that endpoint is notified over, and the topic (by ARN and by name) the
//! notifications are published to — so an environment configured with them has
//! every event this service records about it published to that topic, and a
//! client subscribed to the topic receives it without reading the event log.
//!
//! The delivery is REAL: the publish is an `SNS:Publish` into the emulated
//! Amazon SNS over the in-process callback, so a queue subscribed to the topic
//! receives the notification through Amazon SNS's own delivery, and the topic
//! Elastic Beanstalk creates for a configured endpoint is a topic Amazon SNS
//! really holds. Nothing here writes to a subscriber directly.
//!
//! It is also ASYNCHRONOUS, the way the real service's is: the call that records
//! an event queues the notification it owes ([`PendingEventNotification`]) and
//! the publish is made once the store's lock is released, bounded per request.
//! What one request does not reach stays queued for the next one.
//!
//! Split out of [`super`], which holds the environments and the event log the
//! notifications are about.

use super::*;

/// The configuration-option namespace Elastic Beanstalk's published option
/// reference puts an environment's event-notification settings in.
pub const NAMESPACE_SNS_TOPICS: &str = "aws:elasticbeanstalk:sns:topics";

/// See [`NAMESPACE_SNS_TOPICS`]: "the endpoint where you want to be notified of
/// important events affecting your application".
pub const OPTION_NOTIFICATION_ENDPOINT: &str = "Notification Endpoint";

/// See [`NAMESPACE_SNS_TOPICS`]: "the protocol used to send notifications to
/// your endpoint".
pub const OPTION_NOTIFICATION_PROTOCOL: &str = "Notification Protocol";

/// See [`NAMESPACE_SNS_TOPICS`]: the ARN of the topic the notifications are
/// published to.
pub const OPTION_NOTIFICATION_TOPIC_ARN: &str = "Notification Topic ARN";

/// See [`NAMESPACE_SNS_TOPICS`]: the name of that topic.
pub const OPTION_NOTIFICATION_TOPIC_NAME: &str = "Notification Topic Name";

/// How many notifications one store holds waiting to be published.
///
/// A bound on what one client's lifecycle calls can queue: the notifications are
/// drained by every call this service serves, so the queue is only ever as long
/// as one request's own events — this is the backstop for a store nothing drains,
/// and the OLDEST notification is dropped rather than the newest, so a queue at
/// the bound still delivers what just happened.
const MAX_PENDING_NOTIFICATIONS: usize = 500;

/// How many notifications ONE request publishes. Each publish is an in-process
/// call into Amazon SNS that fans out again to the topic's subscribers, so the
/// work one request can start is capped; what is left stays queued for the next
/// call.
const MAX_NOTIFICATIONS_PER_REQUEST: usize = 50;

/// The longest `Subject` Amazon SNS accepts: "Subjects must be … less than 100
/// characters long" (`PublishInput$Subject`), so a notification about a long
/// event message carries as much of it as a subject line can.
const MAX_SUBJECT_CHARS: usize = 99;

/// The first words of the subject line an Elastic Beanstalk notification carries,
/// before the event it is about.
const SUBJECT_PREFIX: &str = "AWS Elastic Beanstalk Notification - ";

/// The name of the topic Elastic Beanstalk creates for an environment whose
/// configuration names a notification endpoint but no topic — the form the
/// service's own notification topic is named in, for the environment it belongs
/// to.
fn default_topic_name(environment_name: &str) -> String {
    format!("ElasticBeanstalkNotifications-Environment-{environment_name}")
}

/// One recorded event whose notification has not been published yet.
#[derive(Debug, Clone)]
pub struct PendingEventNotification {
    /// The environment the event is about, whose configuration names where the
    /// notification goes. Resolved when the notification is published, not when
    /// it is queued, so an event recorded before the environment's topic existed
    /// is still delivered to it.
    pub environment_id: String,
    /// The event itself, as the log holds it.
    pub event: Event,
}

/// Where one environment's notifications go: the topic, and the endpoint Elastic
/// Beanstalk subscribes to it when it has to create one.
#[derive(Debug, Clone)]
pub struct NotificationTarget {
    /// The Region the environment was created in — the Region its topic lives
    /// in, since a publish only reaches "topics and endpoints in the same Amazon
    /// Web Services Region".
    pub region: String,
    /// The topic the configuration names, if it names one.
    pub topic_arn: Option<String>,
    /// The name the topic has, or the name Elastic Beanstalk creates it under.
    pub topic_name: String,
    /// The endpoint to subscribe to a topic this service creates.
    pub endpoint: Option<String>,
    /// The protocol that endpoint is subscribed over.
    pub protocol: Option<String>,
}

/// Whether these option settings configure event notifications at all: they name
/// a topic to publish to, or an endpoint to notify (which Elastic Beanstalk
/// creates a topic for). An option set to no value configures nothing.
fn notifications_configured(settings: &[ConfigurationOptionSetting]) -> bool {
    [OPTION_NOTIFICATION_TOPIC_ARN, OPTION_NOTIFICATION_ENDPOINT]
        .iter()
        .any(|option| {
            option_value(settings, NAMESPACE_SNS_TOPICS, option).is_some_and(|value| !value.is_empty())
        })
}

/// The non-empty value of one notification option in `settings`.
fn configured(settings: &[ConfigurationOptionSetting], option: &str) -> Option<String> {
    option_value(settings, NAMESPACE_SNS_TOPICS, option)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

impl ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState {
    /// Queue the notification `event` owes, if the environment it is about has
    /// notifications configured.
    ///
    /// Called by [`Self::record_event_at`], the one place an event is appended,
    /// so every event of an environment configured for notifications is
    /// delivered — including the completions the CLOCK makes due, which are
    /// recorded through the same seam.
    pub(super) fn queue_event_notification(&mut self, event: &Event) {
        let Some(environment_id) = event.subject.environment_id.as_deref() else {
            // An event about an application, a version or a template is not an
            // environment event: notifications are configured on an
            // environment, so there is no topic for one.
            return;
        };
        let configured = self
            .environments
            .get(environment_id)
            .is_some_and(|environment| notifications_configured(&environment.option_settings));
        if !configured {
            return;
        }
        if self.pending_notifications.len() >= MAX_PENDING_NOTIFICATIONS {
            // The oldest is dropped, so the queue at its bound still carries the
            // event that just happened.
            ::tracing::warn!(
                environment = %environment_id,
                "this store holds as many unpublished environment notifications as it keeps, so \
                 the oldest is dropped",
            );
            self.pending_notifications.remove(0);
        }
        self.pending_notifications.push(PendingEventNotification {
            environment_id: environment_id.to_string(),
            event: event.clone(),
        });
    }

    /// Take at most `limit` notifications off the queue, oldest first.
    ///
    /// Taken, not read: a notification this returns is gone from the queue, so
    /// no later call publishes it a second time.
    fn take_pending_notifications(&mut self, limit: usize) -> Vec<PendingEventNotification> {
        let taken = limit.min(self.pending_notifications.len());
        self.pending_notifications.drain(..taken).collect()
    }

    /// Where the environment `environment_id`'s notifications go, or `None` when
    /// its configuration names neither a topic nor an endpoint — or when the
    /// store no longer holds the environment at all.
    ///
    /// Read off the environment's RESOLVED configuration, the set in force on it
    /// ([`Self::resolved_configuration_settings`]), so the protocol an endpoint
    /// is subscribed over is the catalog's default when nothing set it.
    pub fn environment_notification_target(
        &self,
        environment_id: &str,
    ) -> Option<NotificationTarget> {
        let environment = self.environments.get(environment_id)?;
        let settings = self.resolved_configuration_settings(
            &environment.region,
            environment.solution_stack_name.as_deref(),
            environment.platform_arn.as_deref(),
            &environment.option_settings,
        );
        let topic_arn = configured(&settings, OPTION_NOTIFICATION_TOPIC_ARN);
        let endpoint = configured(&settings, OPTION_NOTIFICATION_ENDPOINT);
        if topic_arn.is_none() && endpoint.is_none() {
            return None;
        }
        Some(NotificationTarget {
            region: environment.region.clone(),
            topic_arn,
            topic_name: configured(&settings, OPTION_NOTIFICATION_TOPIC_NAME)
                .unwrap_or_else(|| default_topic_name(&environment.environment_name)),
            endpoint,
            protocol: configured(&settings, OPTION_NOTIFICATION_PROTOCOL),
        })
    }

    /// Record the topic Elastic Beanstalk created for the environment
    /// `environment_id` on that environment's configuration, as the service's own
    /// `Notification Topic ARN` and `Notification Topic Name`.
    ///
    /// The topic is part of what the environment is made of, so the configuration
    /// a client reads back names it: that is what makes a second notification
    /// publish to the topic the first one created rather than create another.
    ///
    /// The topic is owed by the CALL that configured the endpoint — and it is
    /// that call's own pass that creates it, because the events it recorded are
    /// what the pass publishes — so this is the provisioning of a resource a
    /// write already made due, not a change a later read caused.
    fn record_notification_topic(&mut self, environment_id: &str, arn: &str, name: &str) {
        let Some(environment) = self.environments.get_mut(environment_id) else {
            return;
        };
        let created = [
            (OPTION_NOTIFICATION_TOPIC_ARN, arn),
            (OPTION_NOTIFICATION_TOPIC_NAME, name),
        ]
        .map(|(option, value)| ConfigurationOptionSetting {
            resource_name: None,
            namespace: Some(NAMESPACE_SNS_TOPICS.to_string()),
            option_name: Some(option.to_string()),
            value: Some(value.to_string()),
        });
        environment.option_settings = merge_option_settings(&environment.option_settings, &created);
    }
}

/// Publish the notifications this store's environments owe, and create the topic
/// an environment configured with an endpoint alone needs.
///
/// Every operation this service serves calls this once it has answered (see
/// `crate::service::query_error_envelope`), so a notification is delivered by the
/// same call that recorded the event — and a completion the CLOCK made due is
/// delivered by whichever call materialised it.
///
/// The store's lock is taken in short critical sections and NEVER held across an
/// await: each publish is an in-process call into Amazon SNS, which must not run
/// under this service's lock. Nothing is dialled outside this emulator — the
/// client's transport is the framework's in-process callback, so both the
/// `CreateTopic` and the `Publish` are served by the Amazon SNS this emulator
/// mounts, and the endpoint a configuration names is handed to that service as a
/// subscription rather than connected to from here.
pub async fn deliver_environment_notifications<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
) {
    let Some(state) = service_state(engine) else {
        // No store: the handler that answered this request has already reported
        // the store's absence with its own modelled server fault.
        return;
    };
    let owed = state
        .lock()
        .take_pending_notifications(MAX_NOTIFICATIONS_PER_REQUEST);
    if owed.is_empty() {
        return;
    }
    for notification in owed {
        let Some(target) = state
            .lock()
            .environment_notification_target(&notification.environment_id)
        else {
            // The environment's configuration no longer names anywhere to
            // notify, so there is nothing this notification is owed to.
            continue;
        };
        let topic_arn = match target.topic_arn.clone() {
            Some(arn) => arn,
            None => {
                let Some(arn) = create_notification_topic(ctx, &target).await else {
                    continue;
                };
                state.lock().record_notification_topic(
                    &notification.environment_id,
                    &arn,
                    &target.topic_name,
                );
                arn
            }
        };
        publish_notification(ctx, &target.region, &topic_arn, &notification).await;
    }
}

/// Create the topic an environment's configured notification endpoint is
/// notified through, and subscribe that endpoint to it. Answers the topic's ARN,
/// or `None` when Amazon SNS would not create it.
///
/// `CreateTopic` is idempotent — it "creates a topic to which notifications can
/// be published", and a name the account already holds answers that topic's ARN
/// — so this cannot end up with two topics for one environment even if the
/// environment's configuration was never written back.
async fn create_notification_topic(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    target: &NotificationTarget,
) -> Option<String> {
    let client = sns_client(ctx.callback(), &target.region);
    let created = client.create_topic().name(&target.topic_name).send().await;
    let topic_arn = match created {
        Ok(answer) => answer.topic_arn,
        Err(failure) => {
            ::tracing::warn!(
                topic = %target.topic_name,
                region = %target.region,
                detail = %::aws_smithy_types::error::display::DisplayErrorContext(&failure),
                "the notification topic this environment's configuration asks for could not be \
                 created, so its events are not published",
            );
            return None;
        }
    }?;
    // "Notification Endpoint" with "Notification Protocol": the endpoint to
    // notify, over the protocol it is notified on, which is what the created
    // topic exists to reach.
    if let Some(endpoint) = target.endpoint.as_deref() {
        let subscribed = client
            .subscribe()
            .topic_arn(&topic_arn)
            .set_protocol(target.protocol.clone())
            .endpoint(endpoint)
            .send()
            .await;
        if let Err(failure) = subscribed {
            ::tracing::warn!(
                topic = %topic_arn,
                detail = %::aws_smithy_types::error::display::DisplayErrorContext(&failure),
                "the endpoint this environment's configuration names could not be subscribed to \
                 its notification topic",
            );
        }
    }
    Some(topic_arn)
}

/// Publish one environment event to `topic_arn`.
///
/// A delivery failure is LOGGED, not raised: the notification is Elastic
/// Beanstalk's own, made after the call that recorded the event has answered, so
/// a topic that has been deleted or that refuses the publish cannot fail a
/// client's lifecycle call. Nothing is requeued — delivery is at-most-once.
async fn publish_notification(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    topic_arn: &str,
    notification: &PendingEventNotification,
) {
    let published = sns_client(ctx.callback(), region)
        .publish()
        .topic_arn(topic_arn)
        .subject(notification_subject(&notification.event))
        .message(notification_message(&notification.event))
        .send()
        .await;
    if let Err(failure) = published {
        ::tracing::warn!(
            topic = %topic_arn,
            environment = %notification.environment_id,
            detail = %::aws_smithy_types::error::display::DisplayErrorContext(&failure),
            "this environment's notification could not be published to its topic",
        );
    }
}

/// The subject line one event's notification carries: the service's own prefix
/// and the event, held inside what Amazon SNS accepts as a `Subject` — at most
/// [`MAX_SUBJECT_CHARS`] characters, with "no line breaks or control characters".
fn notification_subject(event: &Event) -> String {
    let line: String = event
        .message
        .chars()
        .map(|character| if character.is_control() { ' ' } else { character })
        .collect();
    format!("{SUBJECT_PREFIX}{line}")
        .chars()
        .take(MAX_SUBJECT_CHARS)
        .collect()
}

/// The body one event's notification carries: when it happened, what happened,
/// and which environment of which application it happened to — the event as the
/// log holds it, with the resources it is about named beside it, so a subscriber
/// reads the notification without calling `DescribeEvents`.
fn notification_message(event: &Event) -> String {
    let mut body = String::new();
    if let Ok(at) = event
        .event_date
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
    {
        body.push_str(&format!("Timestamp: {at}\n"));
    }
    body.push_str(&format!("Message: {}\n", event.message));
    if let Some(environment_name) = event.subject.environment_name.as_deref() {
        body.push_str(&format!("\nEnvironment: {environment_name}\n"));
    }
    if let Some(application_name) = event.subject.application_name.as_deref() {
        body.push_str(&format!("Application: {application_name}\n"));
    }
    body
}

/// An Amazon SNS client over the in-process callback into this same emulator —
/// the framework's documented seam for calling another emulated service, so no
/// network client is configured and this crate names no other service's crate.
///
/// `region` is the environment's own Region, because a publish only reaches
/// "topics and endpoints in the same Amazon Web Services Region".
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
        // The transport is in-process: there is nothing to retry, time out or
        // watch for a stalled stream, and each of those would need a sleep
        // implementation this client is not given. The same goes for caching the
        // identity: the credentials are handed over directly.
        .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_sns::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One event, for the subject and body writers below.
    fn event(message: &str) -> Event {
        Event {
            sequence: 1,
            event_date: now(),
            message: message.to_string(),
            severity: SEVERITY_INFO.to_string(),
            request_id: "request".to_string(),
            subject: EventSubject {
                application_name: Some("app".to_string()),
                environment_name: Some("env".to_string()),
                environment_id: Some("e-1".to_string()),
                ..Default::default()
            },
        }
    }

    /// The subject line stays inside what Amazon SNS accepts, whatever the event
    /// message is: short enough, single line, no control characters.
    #[test]
    fn a_subject_line_is_one_sns_accepts() {
        for case in [
            "createEnvironment is starting.",
            "Environment health has transitioned from Pending to Ok.",
            &"a very long event message ".repeat(20),
            "a message\nbroken\tover lines",
        ] {
            let subject = notification_subject(&event(case));
            assert!(
                subject.chars().count() <= MAX_SUBJECT_CHARS,
                "case {case:?}: expected at most {MAX_SUBJECT_CHARS} characters, got \
                 {subject:?}"
            );
            assert!(
                !subject.chars().any(char::is_control),
                "case {case:?}: expected no control characters, got {subject:?}"
            );
            assert!(
                subject.starts_with(SUBJECT_PREFIX),
                "case {case:?}: expected the service's own prefix, got {subject:?}"
            );
        }
    }

    /// The body reports the event and the resources it is about.
    #[test]
    fn a_notification_body_reports_the_event_and_its_environment() {
        let case = "createEnvironment is starting.";
        let body = notification_message(&event(case));
        for expected in [
            "Message: createEnvironment is starting.",
            "Environment: env",
            "Application: app",
            "Timestamp: ",
        ] {
            assert!(
                body.contains(expected),
                "case {case:?}: expected {expected:?} in the body, got {body:?}"
            );
        }
    }

    /// Notifications are configured by naming a topic or an endpoint; an option
    /// set to no value names neither.
    #[test]
    fn a_configuration_naming_neither_a_topic_nor_an_endpoint_configures_nothing() {
        // (case, option, value, configured)
        let rows: [(&str, &str, &str, bool); 5] = [
            (
                "a topic ARN",
                OPTION_NOTIFICATION_TOPIC_ARN,
                "arn:aws:sns:us-west-2:123456789012:events",
                true,
            ),
            (
                "an endpoint",
                OPTION_NOTIFICATION_ENDPOINT,
                "arn:aws:sqs:us-west-2:123456789012:events",
                true,
            ),
            ("an empty topic ARN", OPTION_NOTIFICATION_TOPIC_ARN, "", false),
            ("an empty endpoint", OPTION_NOTIFICATION_ENDPOINT, "", false),
            (
                "a protocol alone",
                OPTION_NOTIFICATION_PROTOCOL,
                "sqs",
                false,
            ),
        ];
        for (case, option, value, expected) in rows {
            let settings = [ConfigurationOptionSetting {
                resource_name: None,
                namespace: Some(NAMESPACE_SNS_TOPICS.to_string()),
                option_name: Some(option.to_string()),
                value: Some(value.to_string()),
            }];
            assert_eq!(
                notifications_configured(&settings),
                expected,
                "case {case:?}: expected configured={expected}"
            );
        }
    }
}
