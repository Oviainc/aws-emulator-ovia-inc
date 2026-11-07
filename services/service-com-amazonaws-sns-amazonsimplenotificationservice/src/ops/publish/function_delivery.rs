// SPDX-License-Identifier: CC0-1.0

//! The `lambda`-protocol half of a publish's fan-out: the function a subscription's endpoint
//! names, the event that function is invoked with, and the grant that has to admit the
//! invocation.
//!
//! "lambda – delivery of JSON-encoded message to an Lambda function"
//! (`SubscribeInput$Protocol`), and SNS "delivers the message to each endpoint that is
//! subscribed to the topic" (`Publish` doc) — so a publish to a topic carrying a CONFIRMED
//! `lambda` subscription invokes that function, with the notification wrapped in the
//! `Records` envelope a function's event carries ([`records_envelope`]).
//!
//! The invocation is ASYNCHRONOUS: "Event – Invoke the function asynchronously"
//! (`Invoke$InvocationType`), which is what a publish owes a subscribed function — the
//! publishing call is answered with a message id and does not wait for a handler.
//!
//! A function is a resource of another service, and SNS reaches it as the service principal
//! rather than as the function's owner, so the FUNCTION's own resource-based policy is the
//! only document in force over the invocation and what it states decides it
//! ([`super::resource_policy_allows`]) — exactly the reading the `sqs` half makes of a
//! queue's `Policy`. A function that has granted this service nothing is not invoked, and one
//! whose policy grants it is, so a client repairing the grant sees the next publish arrive.

use crate::state::{self, PublishedMessage};

use super::{
    answered_status, refusal_of, retry_waits, HandOver, QueueDelivery, QueueLocation, RetryBudget,
};

/// The subscription protocol whose endpoint is a Lambda function: "lambda – delivery of
/// JSON-encoded message to an Lambda function" (`SubscribeInput$Protocol`).
pub(super) const LAMBDA_PROTOCOL: &str = "lambda";

/// The ARN namespace of Lambda, whose functions are the subscription endpoint kind this
/// module delivers to.
const LAMBDA_ARN_NAMESPACE: &str = "lambda";

/// What the resource part of a FUNCTION's ARN starts with: `function:<name>` — optionally
/// followed by a version or alias qualifier, which addresses that published version.
const FUNCTION_RESOURCE_PREFIX: &str = "function:";

/// The IAM action a delivery to a subscribed function performs on that function.
const INVOKE_FUNCTION_ACTION: &str = "lambda:InvokeFunction";

/// The error code Lambda's `GetPolicy` answers when there is no policy to report, which is
/// also what a name no function resolves to earns. For a delivery those are the same thing:
/// there is no document granting this service the invocation.
const POLICY_ABSENT_CODE: &str = "ResourceNotFoundException";

/// One invocation this publish owes to one function subscribed to the topic.
///
/// The counterpart of [`QueueDelivery`] for the `lambda` protocol: carried out of the store
/// as a record and performed with the store's lock released, because the delivery is not part
/// of the publishing call's answer.
#[derive(Debug, Clone)]
pub(super) struct FunctionDelivery {
    /// The subscription that owes the delivery: the log names it when it cannot be made, and
    /// its own dead-letter queue is where the event is parked then.
    subscription_arn: String,
    /// The function the subscription's endpoint ARN names.
    target: FunctionLocation,
    /// The topic the message was published to: the source the function's policy may scope its
    /// grant to (`aws:SourceArn`).
    topic_arn: String,
    /// The event the function is invoked with.
    payload: ::std::sync::Arc<str>,
    /// Where this event is parked when the invocation cannot be made: the dead-letter queue
    /// the subscription's own `RedrivePolicy` names, or `None` when it asks for no redrive.
    dead_letter: Option<QueueLocation>,
    /// The ordering members of the published message, carried only so a dead-letter copy
    /// reaches a FIFO queue, which takes no message without a group.
    message_group_id: Option<String>,
    message_deduplication_id: Option<String>,
    /// What the TOPIC asks to have logged about this delivery, and `None` when it asks for
    /// nothing ([`crate::state::delivery_status`]).
    logged: Option<state::delivery_status::Logged>,
}

impl FunctionDelivery {
    /// The invocation `subscription` owes for `published`, whose body over this protocol is
    /// `protocol_body`, or `None` when its endpoint names no function.
    ///
    /// `logged` is what the topic asks to have logged about a delivery over this protocol,
    /// carried with the invocation so the record that follows it needs nothing from the store.
    pub(super) fn of(
        subscription: &state::SubscriptionRecord,
        published: &PublishedMessage,
        protocol_body: &str,
        logged: Option<state::delivery_status::Logged>,
    ) -> Option<Self> {
        let endpoint = subscription.endpoint.as_deref()?;
        let target = FunctionLocation::of_function_arn(endpoint)?;
        Some(Self {
            subscription_arn: subscription.arn.clone(),
            target,
            topic_arn: published.topic_arn.clone(),
            payload: ::std::sync::Arc::from(records_envelope(
                published,
                protocol_body,
                &subscription.arn,
            )),
            // "RedrivePolicy – When specified, sends undeliverable messages to the specified
            // Amazon SQS dead-letter queue" — the same attribute of the same subscription
            // the `sqs` half reads, because a redrive is about the subscription rather than
            // about the endpoint kind.
            dead_letter: state::dead_letter_target_arn(&subscription.attributes)
                .as_deref()
                .and_then(QueueLocation::of_queue_arn),
            message_group_id: published.message_group_id.clone(),
            message_deduplication_id: published.message_deduplication_id.clone(),
            logged,
        })
    }

    /// Write the delivery-status record of this invocation, where the topic asked for one.
    ///
    /// The record reports an invocation that has already been attempted, so it is written
    /// after the attempt and never instead of it.
    pub(super) async fn record_status(
        &self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        invoked: bool,
    ) {
        let Some(logged) = self.logged.as_ref() else {
            return;
        };
        logged
            .record(
                ctx,
                &self.subscription_arn,
                &self.topic_arn,
                &self.target.function_arn,
                invoked,
            )
            .await;
    }

    /// How many bytes this invocation hands to the function: what it costs the request's
    /// delivery budget.
    pub(super) fn payload_len(&self) -> usize {
        self.payload.len()
    }

    /// The copy of this event that is held in the subscription's dead-letter queue when the
    /// function could not be invoked, or `None` when the subscription asks for no redrive.
    ///
    /// The copy is the event the FUNCTION itself would have received, because that is the
    /// message that could not be delivered — the same choice the `sqs` half makes.
    pub(super) fn dead_letter_copy(&self) -> Option<QueueDelivery> {
        let dead_letter = self.dead_letter.clone()?;
        Some(QueueDelivery {
            subscription_arn: self.subscription_arn.clone(),
            target: dead_letter,
            topic_arn: self.topic_arn.clone(),
            body: ::std::sync::Arc::clone(&self.payload),
            attributes: Vec::new(),
            message_group_id: self.message_group_id.clone(),
            message_deduplication_id: self.message_deduplication_id.clone(),
            dead_letter: None,
            // The delivery this copy stands in for has already been reported, so the copy
            // asks for no record of its own.
            logged: None,
        })
    }
}

/// The FUNCTION one subscription's deliveries invoke.
#[derive(Debug, Clone)]
struct FunctionLocation {
    /// The function's own ARN, as the subscription's endpoint names it: the name the
    /// invocation addresses and the RESOURCE its policy is asked about.
    function_arn: String,
    /// The Region the function is in: the one its own ARN names.
    region: String,
}

impl FunctionLocation {
    /// The function `arn` names, or `None` when `arn` is not the ARN of a function.
    fn of_function_arn(arn: &str) -> Option<Self> {
        let parsed = ::service_common::arn::parse(arn)?;
        if parsed.vendor != LAMBDA_ARN_NAMESPACE
            || !parsed.resource.starts_with(FUNCTION_RESOURCE_PREFIX)
        {
            // A function's ARN names it under the `function` resource type; an ARN of
            // another vendor, or of another Lambda resource (a layer, an event source
            // mapping), addresses no function.
            return None;
        }
        let region = ::service_common::arn::region_of(arn, LAMBDA_ARN_NAMESPACE)?;
        Some(Self {
            function_arn: arn.to_string(),
            region: region.to_string(),
        })
    }
}

/// The functions ONE publishing request invokes.
///
/// It remembers what each function of the fan-out answered — the client its Region is reached
/// through and the policy the function itself reports — so a request that owes several events
/// to one function reads that function once. Both tables are bounded by the same
/// [`super::MAX_DELIVERIES_PER_REQUEST`] the deliveries are.
pub(super) struct Functions<'ctx, 'op> {
    ctx: &'ctx smithy_server_runtime::OperationContext<'op>,
    /// One client per Region: every function of one Region is reached through the same
    /// endpoint, and a publish rarely fans out across Regions.
    clients: Vec<(String, ::aws_sdk_lambda::Client)>,
    /// The resource-based policy each function answered with, keyed by its ARN.
    policies: Vec<(String, FunctionPolicy)>,
}

/// What a function answered when its resource-based policy was read.
#[derive(Debug, Clone)]
enum FunctionPolicy {
    /// The function answered: the policy document it carries, or `None` when it carries none
    /// — which is also the answer a name no function resolves to earns, because for a
    /// delivery those are the same absence of a grant.
    Stated(Option<String>),
    /// The read failed in a way nothing can be concluded from, so no verdict is read off a
    /// document this never saw.
    Unreadable,
}

impl<'ctx, 'op> Functions<'ctx, 'op> {
    pub(super) fn new(ctx: &'ctx smithy_server_runtime::OperationContext<'op>) -> Self {
        Self {
            ctx,
            clients: Vec::new(),
            policies: Vec::new(),
        }
    }

    /// The client `region`'s functions are reached through, built on first use.
    fn client(&mut self, region: &str) -> ::aws_sdk_lambda::Client {
        if let Some((_, client)) = self.clients.iter().find(|(held, _)| held == region) {
            return client.clone();
        }
        let fresh = lambda_client(self.ctx, region);
        self.clients.push((region.to_string(), fresh.clone()));
        fresh
    }

    /// The resource-based policy the function at `function_arn` reports, read at most once
    /// per request.
    async fn policy(
        &mut self,
        client: &::aws_sdk_lambda::Client,
        function_arn: &str,
    ) -> FunctionPolicy {
        if let Some((_, policy)) = self.policies.iter().find(|(held, _)| held == function_arn) {
            return policy.clone();
        }
        let read = function_policy(client, function_arn).await;
        self.policies
            .push((function_arn.to_string(), read.clone()));
        read
    }

    /// Invoke the function `delivery` names with the event it carries, and report whether the
    /// invocation was accepted.
    ///
    /// An invocation the function could not take YET is retried along the phases the delivery
    /// policy publishes ([`super::retry_waits`]), within what `waiting` leaves the request — a
    /// function inside the activation window its own `State` reports is exactly that, "During
    /// this time, you can't invoke or modify the function" (`CreateFunction`) — while one it
    /// will not take is not retried at all, because another attempt earns the same answer.
    ///
    /// `false` is every way the event did not reach the function: a function whose own policy
    /// does not grant this service the invocation, a policy that could not be read, and an
    /// invocation the function's service refused — the client errors a redrive exists to make
    /// visible.
    pub(super) async fn invoke(
        &mut self,
        delivery: &FunctionDelivery,
        waiting: &mut RetryBudget,
    ) -> bool {
        let mut waits = retry_waits();
        loop {
            match self.attempt_invoke(delivery).await {
                HandOver::Taken => return true,
                HandOver::Refused => return false,
                HandOver::NotYet => {}
            }
            let Some(wait) = waits.next() else {
                return false;
            };
            if !wait.is_zero() && !waiting.charge(wait) {
                return false;
            }
            if !wait.is_zero() {
                ::tokio::time::sleep(wait).await;
            }
        }
    }

    /// ONE attempt at invoking the function `delivery` names with the event it carries.
    async fn attempt_invoke(&mut self, delivery: &FunctionDelivery) -> HandOver {
        let client = self.client(&delivery.target.region);
        let policy = match self.policy(&client, &delivery.target.function_arn).await {
            FunctionPolicy::Stated(policy) => policy,
            FunctionPolicy::Unreadable => return HandOver::Refused,
        };
        if !super::resource_policy_allows(
            policy.as_deref(),
            INVOKE_FUNCTION_ACTION,
            &delivery.target.function_arn,
            &delivery.topic_arn,
        ) {
            ::tracing::warn!(
                subscription = %delivery.subscription_arn,
                function = %delivery.target.function_arn,
                "the subscribed function's resource-based policy does not allow this service \
                 to invoke it, so the notification is withheld"
            );
            return HandOver::Refused;
        }
        let answered = client
            .invoke()
            .function_name(&delivery.target.function_arn)
            // The publishing call is answered before the handler runs, so the invocation is
            // the asynchronous one: "Event – Invoke the function asynchronously"
            // (`Invoke$InvocationType`).
            .invocation_type(::aws_sdk_lambda::types::InvocationType::Event)
            .payload(::aws_sdk_lambda::primitives::Blob::new(
                delivery.payload.as_bytes(),
            ))
            .send()
            .await;
        if let Err(error) = answered {
            let status = answered_status(&error);
            ::tracing::warn!(
                subscription = %delivery.subscription_arn,
                function = %delivery.target.function_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a subscribed function could not be invoked"
            );
            return refusal_of(status);
        }
        HandOver::Taken
    }
}

/// What the function at `function_arn` answered when its resource-based policy was read.
///
/// A function that carries no policy and a name that resolves to no function both answer
/// [`POLICY_ABSENT_CODE`], and for a delivery those are the same thing: there is no document
/// granting this service the invocation, so that answer is read as the absence of a policy.
/// Every other failure of the read is one nothing can be concluded from.
async fn function_policy(
    client: &::aws_sdk_lambda::Client,
    function_arn: &str,
) -> FunctionPolicy {
    let answered = client.get_policy().function_name(function_arn).send().await;
    let error = match answered {
        Ok(answer) => {
            return FunctionPolicy::Stated(answer.policy().map(ToString::to_string));
        }
        Err(error) => error,
    };
    if ::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(&error)
        == Some(POLICY_ABSENT_CODE)
    {
        return FunctionPolicy::Stated(None);
    }
    ::tracing::warn!(
        function = %function_arn,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
        "a subscribed function's resource-based policy could not be read, so the notification \
         cannot be delivered to it"
    );
    FunctionPolicy::Unreadable
}

/// A Lambda client over the in-process callback into this same emulator.
///
/// Built exactly as the SQS one is ([`super::sqs_client`]) and for the same reasons: the
/// transport is in-process, so there is nothing to retry, time out or watch for a stalled
/// stream, and the credentials are handed over directly.
fn lambda_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_lambda::Client {
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_lambda::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://lambda.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_lambda::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_lambda::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_lambda::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_lambda::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_lambda::Client::from_conf(config)
}

/// The event a subscribed function is invoked with: the notification of `published`, in the
/// `Records` envelope a function's event carries.
///
/// A function's event is a LIST of records, each naming the event's source (`aws:sns`), its
/// version and the subscription the record was delivered to, around the same notification
/// every other enveloped subscriber receives ([`super::notification_members`]) — so the
/// members a handler reads of `Records[0].Sns` are the ones a queue subscriber reads of its
/// message body. One publish owes one record, because it is one message delivered to one
/// subscription.
///
/// The unsubscribe link is the one that cancels THE SUBSCRIPTION THE EVENT IS DELIVERED TO,
/// under the name a function's event carries it by. `MessageAttributes` is always present
/// here, as an empty object for a message published with none: a handler reads the member
/// off every record.
///
/// It carries NO signature members: nothing signs a message in this emulator, and a member
/// that looked like a signature would be one a subscriber could not verify.
fn records_envelope(published: &PublishedMessage, body: &str, subscription_arn: &str) -> String {
    let mut notification = super::notification_members(published, body);
    notification
        .entry(MESSAGE_ATTRIBUTES_MEMBER)
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if let Some(url) = super::unsubscribe_url(&published.topic_arn, subscription_arn) {
        notification.insert(
            UNSUBSCRIBE_URL_MEMBER.to_string(),
            serde_json::Value::from(url),
        );
    }
    let mut record = serde_json::Map::new();
    record.insert(
        "EventSource".to_string(),
        serde_json::Value::from(EVENT_SOURCE),
    );
    record.insert(
        "EventVersion".to_string(),
        serde_json::Value::from(EVENT_VERSION),
    );
    record.insert(
        "EventSubscriptionArn".to_string(),
        serde_json::Value::from(subscription_arn),
    );
    record.insert(
        "Sns".to_string(),
        serde_json::Value::Object(notification),
    );
    let mut envelope = serde_json::Map::new();
    envelope.insert(
        "Records".to_string(),
        serde_json::Value::Array(vec![serde_json::Value::Object(record)]),
    );
    serde_json::Value::Object(envelope).to_string()
}

/// The `EventSource` every record of an SNS event names: the service the event came from.
const EVENT_SOURCE: &str = "aws:sns";

/// The schema version of the record SNS hands a function.
const EVENT_VERSION: &str = "1.0";

/// The member a function's event carries the unsubscribe link under.
const UNSUBSCRIBE_URL_MEMBER: &str = "UnsubscribeUrl";

/// The member the published message's attributes are carried under.
const MESSAGE_ATTRIBUTES_MEMBER: &str = "MessageAttributes";
