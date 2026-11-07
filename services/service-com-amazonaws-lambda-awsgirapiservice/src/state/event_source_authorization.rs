// SPDX-License-Identifier: CC0-1.0

//! Whether the EXECUTION ROLE of the function an event source mapping invokes lets Lambda
//! READ the event source that mapping names.
//!
//! A mapping's poller does not read the source as whoever created the mapping: "Lambda
//! reads items from the event source and invokes the function"
//! ([`CreateEventSourceMapping`](crate::ops::create_event_source_mapping)) ACTING AS the
//! function's execution role — "the Amazon Resource Name (ARN) of the function's execution
//! role" (`FunctionConfiguration$Role`) — so a mapping whose role may not make the poller's
//! own calls is a trigger that can never fire. That is why the question is settled by the
//! call that WIRES the pair together, before anything is persisted: a customer learns that
//! the role is short of a grant from the refusal, instead of from a mapping that polls
//! nothing.
//!
//! The same question is asked again by every PASS of a mapping's poller
//! ([`crate::state::sqs_poller`], [`crate::state::event_source_poller`]), because the
//! policies in force on a role are not frozen when the pair is wired: while they withhold a
//! read the poller makes on the named source, that pass takes nothing out of the source —
//! nothing is delivered, invoked or deleted — and the mapping consumes again as soon as the
//! grant is back in place.
//!
//! The pieces, each held in exactly one place:
//!
//! 1. **who** — the execution role of the function version the mapping would invoke, read
//!    off that version's own stored configuration, so a mapping onto a published version is
//!    judged by the role THAT version carries;
//! 2. **what the poller asks for** — the `(action, resource)` pairs the reads of that KIND
//!    of event source are subject to ([`EVENT_SOURCE_ACCESS`]): the IAM action is the
//!    source service's action namespace and the name of a call the poller makes ON the named
//!    source, and the resource is the `EventSourceArn` the request named. The refusal names
//!    the whole grant a reader of such a source is written, which is wider than that: an
//!    account-wide enumeration of the service's sources is grantable on nothing narrower
//!    than all of them, and a source-discovery read is one a mapping is accepted without, so
//!    those are advice in the sentence rather than conditions of the decision
//!    ([`Read::decided_on_source`]);
//! 3. **what the role may do** — the identity-based documents the IAM store holds for it
//!    (its inline policies and the default version of each managed policy attached to it),
//!    read through IAM's own [`principal_policies`](IamState::principal_policies) accessor
//!    so this service sees exactly what `CreateRole`/`PutRolePolicy` wrote, plus the
//!    permissions boundary that caps them;
//! 4. **the decision** — [`::service_common::iam_authorization`], the tree's ONE evaluation
//!    of policy documents against a request, so this service and every other one handed the
//!    same documents decide alike. How the documents in force combine — an attachment whose
//!    document this account does not hold grants something unreadable, so it withholds a
//!    refusal without overturning an explicit `Deny`, and the boundary caps rather than
//!    grants — is read exactly as IAM's own authorizer reads it, which is the authority on
//!    its own store.
//!
//! The trust half of the question is settled elsewhere and is not repeated here: a role
//! this service cannot assume at all never reaches a mapping, because the operations that
//! STORE a function's execution role already refuse it
//! ([`crate::state::execution_role_is_unassumable`]).
//!
//! Nothing is refused on evidence this emulator does not have. Each of these shows nothing
//! about the role and lets the mapping be created: an engine carrying no IAM store, a role
//! ARN the store holds no role for (it may belong to another account), a role carrying an
//! attachment whose document this account does not hold — an AWS-managed policy might be
//! the very one that carries the grant — and an event source whose kind [`EVENT_SOURCE_ACCESS`]
//! states no reads for. What the documents DO state is decided in IAM's own order: an
//! explicit `Deny` refuses, a grant allows, and policies that state nothing about a read
//! withhold it, because the documents in force on a role are the authority on what it may
//! do.
//!
//! This reading is a service's own because [`::service_common`] cannot hold it: the IAM
//! store's type lives in the IAM service's crate, which depends on `service_common` itself.
//! Only the DECISION is shared, which is the part two services must not answer differently.

use ::service_common::iam_authorization::{self as iam, Decision};

/// The IAM store a function's execution role is resolved against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine this
/// service's store lives in, so a mapping is judged against the very records IAM's handlers
/// wrote rather than a second copy of them.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The IAM store in the SAME shared engine, which is where the account's roles live.
/// `None` means the engine carries no such store — a deployment with no IAM to ask, which
/// is evidence of nothing.
///
/// Reached by [`super::vpc_attachment_authorization`] too, which asks the same store what a
/// function's execution role may do to attach it to a VPC.
pub(super) fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// How Lambda WORDS the refusal of a role that may not make one kind of source's reads.
enum Refusal {
    /// A source whose records are read per SHARD: one sentence naming the source and every
    /// action a reader of it needs, because the grant a customer has to widen is the whole
    /// set rather than the one action that happened to be looked at first.
    Shards {
        /// What that sentence calls the source.
        noun: &'static str,
        /// What closes that sentence after the noun it ends on. The two shard sources word
        /// this one error class differently at the end — one names where the grant is
        /// written, the other stops at the noun — so the tail belongs to the source rather
        /// than to a template shared between them.
        tail: &'static str,
    },
    /// A QUEUE, whose reads are each a call of the queue's own API: one sentence naming the
    /// call the role may not make, and the service it would have been made to. The role is
    /// named as the FUNCTION's execution role, which is whose grant is short.
    Call {
        /// How that sentence names the service.
        service: &'static str,
    },
}

/// ONE action the refusal of an event source's reads names.
struct Read {
    /// The action name, as both the refusal sentence lists it and the IAM action string
    /// names it under the source service's namespace.
    name: &'static str,
    /// Whether the DECISION is made on this action, with the `EventSourceArn` as the
    /// resource.
    ///
    /// `true` for the calls the poller has to make on the NAMED source to consume it, which
    /// is what the wiring call authorizes: a role granted those on that source's ARN is
    /// accepted. `false` for the rest of what the sentence lists — the wider grant AWS's own
    /// managed execution-role policy carries, which includes an account-wide enumeration of
    /// the service's sources that IAM scopes to no single one of them (its only grantable
    /// resource is every source, so a grant of it on one source's ARN does not exist to be
    /// found) and the source-discovery reads a mapping is accepted without. Those are advice
    /// in the sentence, telling a customer the whole grant to write; holding a role to one
    /// of them on the source would refuse a correctly scoped least-privilege policy.
    decided_on_source: bool,
}

/// A call the poller makes on the named source, which the decision is made on.
const fn polls(name: &'static str) -> Read {
    Read {
        name,
        decided_on_source: true,
    }
}

/// An action the refusal advises granting and no mapping is refused for the want of (see
/// [`Read::decided_on_source`]).
const fn advises(name: &'static str) -> Read {
    Read {
        name,
        decided_on_source: false,
    }
}

/// What reading ONE kind of event source takes, and how a role that may not do it is
/// refused.
struct EventSourceAccess {
    /// The ARN vendor field of the sources this entry is about, as
    /// [`crate::state::event_source_vendor`] reads one.
    vendor: &'static str,
    /// The IAM action namespace those actions are named in, which is the source service's
    /// own.
    namespace: &'static str,
    /// The actions the refusal of this kind of source names, in the order it reports them.
    /// Each list is the set of read actions AWS's own managed execution-role policy for that
    /// event source grants (`AWSLambdaKinesisExecutionRole`,
    /// `AWSLambdaDynamoDBExecutionRole`, `AWSLambdaSQSQueueExecutionRole`), which is the
    /// grant the sentence advises; which of them the DECISION is made on is each one's own
    /// ([`Read::decided_on_source`]).
    actions: &'static [Read],
    /// How a withheld read is worded.
    refusal: Refusal,
}

/// What makes a Kinesis `stream/…` ARN name a stream CONSUMER rather than the data stream
/// itself: `EventSourceArn` admits both ("the ARN of the data stream or a stream
/// consumer"), and a consumer is read through the consumer's own calls as well as the
/// stream's, on two different resources — so this module states no reads for one rather
/// than hold a consumer to the stream's own set, which would refuse a correctly granted
/// enhanced fan-out role. It is left to the change that implements consumer polling.
const KINESIS_CONSUMER_SEGMENT: &str = "/consumer/";

/// What the refusal of each KIND of event source names, and which of it the decision is
/// made on.
///
/// Only the sources whose reads are plain resource-level calls on the source itself are
/// here. An Amazon DocumentDB change stream is reached through the cluster's own
/// credentials and network, so what a role has to be granted for it is not a read of the
/// named resource; that source is left to the change that implements its polling.
///
/// A shard source is consumed by the SAME three calls whichever of the two services holds
/// it — describe the source, open an iterator on a shard, get that shard's records — so
/// those three are what a mapping onto one is decided on, and a role granted exactly them on
/// the source is accepted. The other actions in a sentence are the rest of the grant AWS
/// advises writing.
///
/// An Amazon MSK cluster and an Amazon MQ broker are each consumed through a DESCRIBE of the
/// named resource first — Lambda reads the cluster's bootstrap brokers, or the broker's
/// endpoints, before it can connect at all — so that describe is the call a mapping onto one
/// is decided on, and the rest of what AWS's own managed execution-role policy for the
/// source grants (`AWSLambdaMSKExecutionRole`, the Amazon MQ execution-role policy) is the
/// grant the sentence advises.
const EVENT_SOURCE_ACCESS: &[EventSourceAccess] = &[
    EventSourceAccess {
        vendor: super::EVENT_SOURCE_VENDOR_KINESIS,
        namespace: super::EVENT_SOURCE_VENDOR_KINESIS,
        actions: &[
            polls("GetRecords"),
            polls("GetShardIterator"),
            polls("DescribeStream"),
            advises("DescribeStreamSummary"),
            advises("ListShards"),
            advises("ListStreams"),
        ],
        refusal: Refusal::Shards {
            noun: "stream",
            tail: ".",
        },
    },
    EventSourceAccess {
        vendor: super::EVENT_SOURCE_VENDOR_DYNAMODB,
        namespace: super::EVENT_SOURCE_VENDOR_DYNAMODB,
        actions: &[
            polls("GetRecords"),
            polls("GetShardIterator"),
            polls("DescribeStream"),
            advises("ListStreams"),
        ],
        refusal: Refusal::Shards {
            noun: "stream",
            tail: " in IAM.",
        },
    },
    EventSourceAccess {
        vendor: super::EVENT_SOURCE_VENDOR_QUEUE,
        namespace: super::EVENT_SOURCE_VENDOR_QUEUE,
        actions: &[
            polls("ReceiveMessage"),
            polls("DeleteMessage"),
            polls("GetQueueAttributes"),
        ],
        refusal: Refusal::Call { service: "SQS" },
    },
    EventSourceAccess {
        vendor: super::EVENT_SOURCE_VENDOR_KAFKA,
        // An Amazon MSK cluster's own actions are named under the Apache Kafka namespace,
        // which is the vendor field of its ARNs too.
        namespace: super::EVENT_SOURCE_VENDOR_KAFKA,
        actions: &[
            polls("DescribeClusterV2"),
            advises("DescribeCluster"),
            advises("GetBootstrapBrokers"),
            advises("ListScramSecrets"),
        ],
        refusal: Refusal::Call { service: "MSK" },
    },
    EventSourceAccess {
        vendor: super::EVENT_SOURCE_VENDOR_MQ,
        namespace: super::EVENT_SOURCE_VENDOR_MQ,
        actions: &[polls("DescribeBroker")],
        refusal: Refusal::Call { service: "MQ" },
    },
];

impl EventSourceAccess {
    /// The FIRST of the calls this entry is DECIDED on that the role `role_arn` may not
    /// perform on `resource`, or `None` when it may perform all of them — or when nothing of
    /// the kind can be shown (see the module docs for the cases that show nothing).
    ///
    /// The actions the sentence only ADVISES are not asked about
    /// ([`Read::decided_on_source`]).
    fn withheld(&self, iam: &IamState, role_arn: &str, resource: &str) -> Option<&'static str> {
        // The role itself first: a value this account's IAM holds no role under says
        // nothing about what that role may do.
        iam.roles.values().find(|role| role.arn == role_arn)?;
        // The documents in force on the role. The collection is bounded by IAM's own quotas
        // on how many policies one role carries, so this walk is bounded by the store
        // rather than by a number of its own. Each document is read ONCE, because one
        // source takes several actions over the same documents.
        let policies = iam.principal_policies(role_arn)?;
        let documents = iam::prepare_documents(&policies.documents);
        // "A permissions boundary sets the maximum permissions that an identity-based
        // policy can grant to an entity": the boundary CAPS what those policies grant and
        // grants nothing itself. A role carrying none — or one whose document is not held,
        // a ceiling this emulator cannot read — is capped by nothing.
        let boundary = iam
            .permissions_boundary_document(role_arn)
            .map(|document| iam::prepare_documents(::std::slice::from_ref(&document)));
        self.decided_actions().find(|name| {
            call_is_withheld(
                &documents,
                boundary.as_deref(),
                !policies.uncarried.is_empty(),
                &format!("{}:{name}", self.namespace),
                resource,
            )
        })
    }

    /// The calls of this entry a mapping onto the source is decided on.
    fn decided_actions(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.actions
            .iter()
            .filter(|read| read.decided_on_source)
            .map(|read| read.name)
    }

    /// The sentence Lambda refuses the mapping with, for an event source whose read
    /// `withheld` names is one the role may not make.
    ///
    /// A QUEUE's sentence names the role as the FUNCTION's execution role — "The function
    /// execution role does not have permissions to call ReceiveMessage on SQS" — which is
    /// how the service worded this refusal when it was observed. Only the CALL and the
    /// service it would have been made to vary with the request; the rest is fixed text.
    fn message(&self, event_source_arn: &str, withheld: &str) -> String {
        match self.refusal {
            Refusal::Shards { noun, tail } => format!(
                "Cannot access {noun} {event_source_arn}. Please ensure the role can perform \
                 the {actions} Actions on your {noun}{tail}",
                actions = self.action_list()
            ),
            Refusal::Call { service } => format!(
                "The function execution role does not have permissions to call {withheld} on \
                 {service}"
            ),
        }
    }

    /// This entry's action names as the shard-source sentence lists them: comma separated,
    /// with `and` before the last.
    fn action_list(&self) -> String {
        let mut listed = String::new();
        let last = self.actions.len().saturating_sub(1);
        for (index, read) in self.actions.iter().enumerate() {
            if index > 0 {
                listed.push_str(", ");
            }
            if index == last && last > 0 {
                listed.push_str("and ");
            }
            listed.push_str(read.name);
        }
        listed
    }
}

/// What reading the event source `event_source_arn` names takes, or `None` for a source
/// whose reads this module states none for — including a Kinesis stream CONSUMER, which is
/// read on two resources rather than one (see [`KINESIS_CONSUMER_SEGMENT`]).
fn access_of(event_source_arn: &str) -> Option<&'static EventSourceAccess> {
    let vendor = super::event_source_vendor(event_source_arn)?;
    if vendor == super::EVENT_SOURCE_VENDOR_KINESIS
        && event_source_arn.contains(KINESIS_CONSUMER_SEGMENT)
    {
        return None;
    }
    EVENT_SOURCE_ACCESS
        .iter()
        .find(|entry| entry.vendor == vendor)
}

/// How Lambda words the refusal of a role that may not make ONE call Lambda itself would
/// make on the caller's behalf, as the surfaces that stand a resource UP word it: "The
/// provided execution role does not have permissions to call CreateNetworkInterface on EC2"
/// ([`super::vpc_attachment_authorization`]). Only the CALL and the service it would have
/// been made to vary with the request; the rest is fixed text.
///
/// Shared with the gate on the destination an asynchronous invocation record is delivered to
/// ([`super::event_invoke_config`]), so one error class is worded one way. The refusal of a
/// read of a mapping's event SOURCE is a sentence of its own ([`Refusal::Call`]), as that
/// service answered it when it was observed.
pub(super) fn provided_role_refusal_message(action: &str, service: &str) -> String {
    format!("The provided execution role does not have permissions to call {action} on {service}")
}

/// Whether the identity-based documents in force on `role_arn` WITHHOLD the call `action`
/// on `resource`.
///
/// `None` when nothing of the kind can be shown: this account's IAM holds no role under
/// that ARN (it may belong to another account), or the store holds no policies to read for
/// it. The DECISION itself is [`call_is_withheld`], the one reading of the documents in
/// force this module states.
pub(super) fn withholds_call(
    iam: &IamState,
    role_arn: &str,
    action: &str,
    resource: &str,
) -> Option<bool> {
    iam.roles.values().find(|role| role.arn == role_arn)?;
    let policies = iam.principal_policies(role_arn)?;
    let documents = iam::prepare_documents(&policies.documents);
    let boundary = iam
        .permissions_boundary_document(role_arn)
        .map(|document| iam::prepare_documents(::std::slice::from_ref(&document)));
    Some(call_is_withheld(
        &documents,
        boundary.as_deref(),
        !policies.uncarried.is_empty(),
        action,
        resource,
    ))
}

/// Whether `action` on `resource` is withheld by the documents in force, read in IAM's own
/// order.
///
/// `documents` are the identity-based documents the role carries and `boundary` the
/// permissions boundary that caps them, both already prepared so one call can be decided
/// over the same documents as the next. `uncarried` says whether the role carries an
/// attachment whose document this account does not hold.
fn call_is_withheld(
    documents: &[iam::PreparedDocument],
    boundary: Option<&[iam::PreparedDocument]>,
    uncarried: bool,
    action: &str,
    resource: &str,
) -> bool {
    let request = iam::Request::new(action, resource, &[]);
    let stated = iam::identity_decision_prepared(documents, &request);
    // An attachment whose document this account does not hold is a grant this emulator
    // cannot read, so it cannot be the basis of a refusal — but it cannot overturn one
    // either: an explicit `Deny` in a document that IS held wins over every grant,
    // including one whose content is unknown here.
    let granted = stated == Decision::Allowed || (stated != Decision::Denied && uncarried);
    // A call an identity-based policy explicitly DENIES is already withheld on that deny,
    // so no ceiling is read for it.
    let capped = if stated == Decision::Denied {
        Decision::Allowed
    } else {
        boundary.map_or(Decision::Allowed, |boundary| {
            iam::identity_decision_prepared(boundary, &request)
        })
    };
    !granted || capped != Decision::Allowed
}

/// The sentence Lambda refuses a mapping between the event source `event_source_arn` and a
/// function running as `role_arn` with, because the policies in force on that role withhold
/// a read the mapping's poller would make — or `None` when the role may make every one of
/// them, or when nothing of the kind can be shown (see the module docs).
///
/// A READ of both stores: it decides, and changes nothing. The caller refuses with the
/// modelled `InvalidParameterValueException` and persists nothing, which is what makes the
/// refusal and the mapping the only two outcomes of such a request.
#[must_use]
pub fn unreadable_event_source<Eng: Send + Sync + 'static>(
    engine: &Eng,
    role_arn: &str,
    event_source_arn: &str,
) -> Option<String> {
    unreadable_event_source_held(iam_state(engine).as_ref(), role_arn, event_source_arn)
}

/// The SAME decision, for a caller holding the IAM store handle already.
///
/// The pollers ask this of EVERY pass rather than once: Lambda reads the source acting as
/// the role for as long as the mapping polls, so a grant taken away after the pair was
/// wired withholds the reads of the next pass exactly as it would have withheld the wiring
/// ([`unreadable_event_source`]). `None` for the handle is an engine carrying no IAM store,
/// which shows nothing about the role.
#[must_use]
pub(super) fn unreadable_event_source_held(
    iam: Option<&::engine::ServiceState<IamState>>,
    role_arn: &str,
    event_source_arn: &str,
) -> Option<String> {
    let access = access_of(event_source_arn)?;
    let withheld = access.withheld(&iam?.lock(), role_arn, event_source_arn)?;
    Some(access.message(event_source_arn, withheld))
}

#[cfg(test)]
mod tests {
    use super::{access_of, EVENT_SOURCE_ACCESS};

    /// The action list of the sentence is AWS's prose list: comma separated, with `and`
    /// before the last, whatever the length of the catalog entry.
    #[test]
    fn the_sentence_lists_every_action_a_reader_of_the_source_needs() {
        // (the event source, the list the sentence has to carry)
        let rows: [(&str, &str); 2] = [
            (
                "arn:aws:kinesis:us-east-1:000000000000:stream/order-events",
                "GetRecords, GetShardIterator, DescribeStream, DescribeStreamSummary, \
                 ListShards, and ListStreams",
            ),
            (
                "arn:aws:dynamodb:us-east-1:000000000000:table/orders/stream/2026-10-23T11:46:49.000",
                "GetRecords, GetShardIterator, DescribeStream, and ListStreams",
            ),
        ];
        for (event_source, expected) in rows {
            let access = access_of(event_source)
                .unwrap_or_else(|| panic!("case {event_source:?}: no reads are stated for it"));
            let listed = access.action_list();
            assert_eq!(
                listed, expected,
                "case {event_source:?}: expected {expected:?}, got {listed:?}"
            );
        }
    }

    /// A Kinesis stream CONSUMER is read on two resources — the stream's own calls and the
    /// consumer's — so this module states no reads for one, while the data stream of the
    /// same name keeps the stream entry.
    #[test]
    fn a_stream_consumer_is_held_to_no_reads_of_its_own() {
        // (case, the event source, whether reads are stated for it)
        let rows: [(&str, &str, bool); 3] = [
            (
                "a data stream",
                "arn:aws:kinesis:us-east-1:000000000000:stream/order-events",
                true,
            ),
            (
                "a stream consumer of that stream",
                "arn:aws:kinesis:us-east-1:000000000000:stream/order-events/consumer/fanout:1700000000",
                false,
            ),
            (
                "a queue, whose reads are its own API's calls",
                "arn:aws:sqs:us-east-1:000000000000:order-intake",
                true,
            ),
        ];
        for (case, event_source, stated) in rows {
            let read = access_of(event_source).is_some();
            assert_eq!(
                read, stated,
                "case {case:?}: expected reads stated to be {stated}, got {read}"
            );
        }
    }

    /// The whole sentence, which is the source's own: the action list it advises, and the
    /// tail that one source ends on and the other does not.
    #[test]
    fn the_refusal_is_worded_as_the_source_words_that_error_class() {
        // (the event source, the sentence AWS answers for it)
        let rows: [(&str, &str); 2] = [
            (
                "arn:aws:kinesis:us-east-1:000000000000:stream/order-events",
                "Cannot access stream arn:aws:kinesis:us-east-1:000000000000:stream/order-events. \
                 Please ensure the role can perform the GetRecords, GetShardIterator, \
                 DescribeStream, DescribeStreamSummary, ListShards, and ListStreams Actions on \
                 your stream.",
            ),
            (
                "arn:aws:dynamodb:us-east-1:000000000000:table/orders/stream/2026-10-23T11:46:49.000",
                "Cannot access stream arn:aws:dynamodb:us-east-1:000000000000:table/orders/stream/\
                 2026-10-23T11:46:49.000. Please ensure the role can perform the GetRecords, \
                 GetShardIterator, DescribeStream, and ListStreams Actions on your stream in IAM.",
            ),
        ];
        for (event_source, expected) in rows {
            let access = access_of(event_source)
                .unwrap_or_else(|| panic!("case {event_source:?}: no reads are stated for it"));
            let message = access.message(event_source, "GetRecords");
            assert_eq!(
                message, expected,
                "case {event_source:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    /// The decision is made on the calls the poller makes on the named source, and on those
    /// only: the rest of the sentence is the grant it advises writing, asked of no source.
    #[test]
    fn only_the_calls_the_poller_makes_on_the_source_are_decided_on_it() {
        for entry in EVENT_SOURCE_ACCESS {
            let decided: Vec<&str> = entry.decided_actions().collect();
            let listed: Vec<&str> = entry.actions.iter().map(|read| read.name).collect();
            assert!(
                !decided.is_empty(),
                "case {:?}: expected at least one call decided on the source, got none of {listed:?}",
                entry.vendor
            );
            for read in entry.actions {
                let asked = decided.contains(&read.name);
                assert_eq!(
                    asked, read.decided_on_source,
                    "case {:?}: expected {:?} to be decided on the source: {}, got {asked}",
                    entry.vendor, read.name, read.decided_on_source
                );
            }
        }
    }

    /// The three calls consuming a shard source takes are what a mapping onto either shard
    /// source is decided on, so a role scoped to exactly them on the source is accepted.
    #[test]
    fn both_shard_sources_are_decided_on_the_same_three_calls() {
        // (the event source, the calls a mapping onto it is decided on)
        let rows: [(&str, [&str; 3]); 2] = [
            (
                "arn:aws:kinesis:us-east-1:000000000000:stream/order-events",
                ["GetRecords", "GetShardIterator", "DescribeStream"],
            ),
            (
                "arn:aws:dynamodb:us-east-1:000000000000:table/orders/stream/2026-10-23T11:46:49.000",
                ["GetRecords", "GetShardIterator", "DescribeStream"],
            ),
        ];
        for (event_source, expected) in rows {
            let access = access_of(event_source)
                .unwrap_or_else(|| panic!("case {event_source:?}: no reads are stated for it"));
            let decided: Vec<&str> = access.decided_actions().collect();
            assert_eq!(
                decided,
                expected.to_vec(),
                "case {event_source:?}: expected {expected:?}, got {decided:?}"
            );
        }
    }

    /// Each entry is read under the vendor field of the ARNs it is about, and no two
    /// entries claim the same vendor — one source has one set of reads.
    #[test]
    fn each_kind_of_event_source_is_read_under_one_entry() {
        for entry in EVENT_SOURCE_ACCESS {
            let same = EVENT_SOURCE_ACCESS
                .iter()
                .filter(|other| other.vendor == entry.vendor)
                .count();
            assert_eq!(
                same, 1,
                "case {:?}: expected one entry for the vendor, got {same}",
                entry.vendor
            );
            assert!(
                !entry.actions.is_empty(),
                "case {:?}: an entry with no actions states no reads at all",
                entry.vendor
            );
        }
    }
}
