// SPDX-License-Identifier: CC0-1.0

//! The ASYNCHRONOUS-INVOCATION CONFIGURATION slice of this service's state: the settings
//! `PutFunctionEventInvokeConfig` "Configures ... for asynchronous invocation on a
//! function, version, or alias", the record the five operations on that resource read and
//! write, and whether the function's execution role lets Lambda deliver an invocation
//! record to the destinations a configuration names.
//!
//! It lives in its own module rather than in the already long [`crate::state`], which
//! re-exports every item here, so a handler still names them as `crate::state::…`. The
//! records themselves hang off the function
//! ([`crate::state::FunctionRecord::event_invoke_configs`]), one per QUALIFIER a
//! configuration was put on, because the resource is "a function, version, or alias": a
//! configuration put on an alias is its own and never the one the version that alias
//! invokes holds.
//!
//! What the write does and what a read reports are the same stored thing: a put REPLACES
//! the addressed qualifier's configuration whole ("If a configuration already exists for a
//! function, version, or alias, this operation overwrites it. If you exclude any settings,
//! they are removed"), an update changes only the settings its request carries ("To set one
//! option without affecting existing settings for other options, use
//! UpdateFunctionEventInvokeConfig"), and a qualifier nobody has configured holds nothing
//! at all — which is what makes the reads answer the modelled 404 rather than a
//! configuration nobody asked for.

use std::time::SystemTime;

use crate::ops::types::{
    DestinationConfig, GetFunctionEventInvokeConfigOutput, OnFailure, OnSuccess,
};

use super::event_source_authorization::{iam_state, provided_role_refusal_message, withholds_call};

/// How a read of an asynchronous-invocation configuration no qualifier holds is refused:
/// the resource is the configuration rather than the function, so the sentence names the
/// function ARN the request addressed and says the configuration is what is missing.
#[must_use]
pub fn no_event_invoke_config_message(function_arn: &str) -> String {
    format!("The function {function_arn} doesn't have an EventInvokeConfig")
}

/// The `FunctionArn` a configuration is reported on: the function's ARN qualified by the
/// qualifier the configuration belongs to.
///
/// A configuration of the UNPUBLISHED version is reported on the `:$LATEST`-qualified ARN
/// rather than the bare one — AWS's own documented answer to a put that named no qualifier
/// carries `"FunctionArn": "arn:aws:lambda:<region>:<account>:function:<name>:$LATEST"`, and
/// `FunctionArn`'s pattern admits exactly that suffix — because the resource is the
/// configuration OF A QUALIFIER, so the ARN names which one answered.
#[must_use]
pub fn configured_function_arn(function_arn: &str, qualifier: &str) -> String {
    format!("{function_arn}:{qualifier}")
}

/// One asynchronous-invocation configuration, as one qualifier of one function holds it.
///
/// Only the settings a caller actually SET are held: the model gives
/// `MaximumRetryAttempts` and `MaximumEventAgeInSeconds` no default of their own, and the
/// documented behaviour of an event invoked without them ("Lambda retries an asynchronous
/// invocation twice ... It retains events in a queue for up to six hours") is what DELIVERY
/// applies, not a value this resource holds. So a setting nobody stated is absent here and
/// absent from every report of it.
#[derive(Debug, Clone)]
pub struct EventInvokeConfigRecord {
    /// `MaximumRetryAttempts`, when a request set it.
    pub maximum_retry_attempts: Option<i32>,
    /// `MaximumEventAgeInSeconds`, when a request set it.
    pub maximum_event_age_in_seconds: Option<i32>,
    /// The `DestinationConfig$OnSuccess` destination, when a request named one.
    pub on_success: Option<String>,
    /// The `DestinationConfig$OnFailure` destination, when a request named one.
    pub on_failure: Option<String>,
    /// When this configuration was last written, which is what `LastModified` ("The date
    /// and time that the configuration was last updated") reports.
    pub last_modified: SystemTime,
}

impl EventInvokeConfigRecord {
    /// The configuration a PUT of these settings leaves behind: exactly what the request
    /// carried, so a setting it excluded is removed rather than carried over from whatever
    /// the qualifier held before.
    #[must_use]
    pub fn of_request(
        maximum_retry_attempts: Option<i32>,
        maximum_event_age_in_seconds: Option<i32>,
        destination_config: Option<&DestinationConfig>,
        now: SystemTime,
    ) -> Self {
        Self {
            maximum_retry_attempts,
            maximum_event_age_in_seconds,
            on_success: named_destination(destination_config.and_then(|config| {
                config
                    .on_success
                    .as_ref()
                    .and_then(|slot| slot.destination.as_deref())
            })),
            on_failure: named_destination(destination_config.and_then(|config| {
                config
                    .on_failure
                    .as_ref()
                    .and_then(|slot| slot.destination.as_deref())
            })),
            last_modified: now,
        }
    }

    /// Apply an UPDATE of these settings to the configuration this qualifier already
    /// holds: each setting the request carries replaces the held one, and each setting it
    /// does not carry is left exactly as it stands.
    pub fn apply_update(
        &mut self,
        maximum_retry_attempts: Option<i32>,
        maximum_event_age_in_seconds: Option<i32>,
        destination_config: Option<&DestinationConfig>,
        now: SystemTime,
    ) {
        if let Some(attempts) = maximum_retry_attempts {
            self.maximum_retry_attempts = Some(attempts);
        }
        if let Some(age) = maximum_event_age_in_seconds {
            self.maximum_event_age_in_seconds = Some(age);
        }
        // `DestinationConfig` is ONE setting of the request, so a request carrying it
        // states the whole destination configuration: the slot it leaves out is removed,
        // which is also how the member's own pattern lets a caller clear one slot while
        // naming the other (see [`named_destination`]).
        if let Some(config) = destination_config {
            self.on_success = named_destination(
                config
                    .on_success
                    .as_ref()
                    .and_then(|slot| slot.destination.as_deref()),
            );
            self.on_failure = named_destination(
                config
                    .on_failure
                    .as_ref()
                    .and_then(|slot| slot.destination.as_deref()),
            );
        }
        self.last_modified = now;
    }

    /// The `FunctionEventInvokeConfig` the wire reports for this configuration, on the
    /// qualified ARN `function_arn` the request addressed.
    ///
    /// Both destination slots are reported whether or not they carry a destination, which
    /// is how AWS answers a configuration whose on-success slot is unset ("DestinationConfig":
    /// {"OnSuccess": {}, "OnFailure": {"Destination": "..."}}), so a client reads the pair
    /// unconditionally.
    #[must_use]
    pub fn reported(&self, function_arn: String) -> GetFunctionEventInvokeConfigOutput {
        GetFunctionEventInvokeConfigOutput {
            last_modified: Some(::aws_smithy_types::DateTime::from(self.last_modified)),
            function_arn: Some(function_arn),
            maximum_retry_attempts: self.maximum_retry_attempts,
            maximum_event_age_in_seconds: self.maximum_event_age_in_seconds,
            destination_config: Some(DestinationConfig {
                on_success: Some(OnSuccess {
                    destination: self.on_success.clone(),
                }),
                on_failure: Some(OnFailure {
                    destination: self.on_failure.clone(),
                }),
            }),
        }
    }
}

/// The destination ONE slot of a `DestinationConfig` names, or `None` for a slot that
/// names none.
///
/// `DestinationArn`'s own pattern is `^$|kafka://…|arn:…`, so the EMPTY STRING is a value
/// the member admits and the only one that is no resource at all: it is how a caller takes
/// a destination away, and it is held as "no destination" rather than as an ARN of nothing.
fn named_destination(destination: Option<&str>) -> Option<String> {
    destination
        .filter(|destination| !destination.is_empty())
        .map(str::to_string)
}

/// Every destination a request's `DestinationConfig` names, on-success first.
#[must_use]
pub fn requested_destinations(destination_config: Option<&DestinationConfig>) -> Vec<String> {
    let Some(config) = destination_config else {
        return Vec::new();
    };
    [
        config
            .on_success
            .as_ref()
            .and_then(|slot| slot.destination.as_deref()),
        config
            .on_failure
            .as_ref()
            .and_then(|slot| slot.destination.as_deref()),
    ]
    .into_iter()
    .filter_map(named_destination)
    .collect()
}

/// What delivering an invocation record to ONE KIND of destination takes, and how a role
/// that may not do it is refused.
struct DestinationWrite {
    /// The ARN vendor field of the destinations this entry is about, as
    /// [`service_common::arn::vendor`] reads one.
    vendor: &'static str,
    /// The IAM action namespace that call is named in, which is the destination service's
    /// own.
    namespace: &'static str,
    /// The call Lambda makes ON the named destination to deliver the record there.
    action: &'static str,
    /// How the refusal names the service that call is made to.
    service: &'static str,
}

/// What a write to each KIND of destination takes.
///
/// One entry per destination kind `FunctionEventInvokeConfig$DestinationConfig` documents
/// — "Function - The Amazon Resource Name (ARN) of a Lambda function. Queue - The ARN of a
/// standard SQS queue. Bucket - The ARN of an Amazon S3 bucket. Topic - The ARN of a
/// standard SNS topic. Event Bus - The ARN of an Amazon EventBridge event bus." — whose
/// write is a plain resource-level call on the destination the request named, with the IAM
/// action being the destination service's action namespace and the name of that call.
///
/// An Amazon S3 BUCKET is deliberately not here: Lambda writes the record as an object
/// INSIDE the bucket, and `s3:PutObject` is grantable only on object ARNs, so holding a
/// role to that call on the bucket ARN the request names would refuse a correctly scoped
/// least-privilege policy. A destination kind with no entry is evidence of nothing and
/// refuses no configuration; it is left to the change that implements delivery to one.
const DESTINATION_WRITES: &[DestinationWrite] = &[
    DestinationWrite {
        vendor: "sqs",
        namespace: "sqs",
        action: "SendMessage",
        service: "SQS",
    },
    DestinationWrite {
        vendor: "sns",
        namespace: "sns",
        action: "Publish",
        service: "SNS",
    },
    DestinationWrite {
        vendor: "events",
        namespace: "events",
        action: "PutEvents",
        service: "EventBridge",
    },
    DestinationWrite {
        vendor: "lambda",
        namespace: "lambda",
        action: "InvokeFunction",
        service: "Lambda",
    },
];

/// What writing to the destination `destination` takes, or `None` for a destination kind
/// this module states no write for (see [`DESTINATION_WRITES`]).
fn write_of(destination: &str) -> Option<&'static DestinationWrite> {
    let vendor = service_common::arn::vendor(destination)?;
    DESTINATION_WRITES
        .iter()
        .find(|entry| entry.vendor == vendor)
}

/// The sentence Lambda refuses a configuration naming one of `destinations` with, because
/// the policies in force on the function's execution role `role_arn` withhold the write
/// Lambda would make to deliver a record there — or `None` when the role may make every one
/// of those writes, or when nothing of the kind can be shown.
///
/// Lambda delivers an invocation record ACTING AS the function's execution role, so a
/// configuration whose role may not write the destination is a destination no record could
/// ever reach: the question is settled by the call that CONFIGURES the pair, before anything
/// is persisted, exactly as it is settled for the event source a mapping reads
/// ([`super::event_source_authorization`]) — and on the same evidence, so an engine carrying
/// no IAM store, a role ARN that store holds no role for, and a role carrying a grant this
/// emulator cannot read all refuse nothing.
///
/// A READ of both stores: it decides, and changes nothing.
#[must_use]
pub fn unwritable_destination<Eng: Send + Sync + 'static>(
    engine: &Eng,
    role_arn: &str,
    destinations: &[String],
) -> Option<String> {
    let iam = iam_state(engine)?;
    destinations.iter().find_map(|destination| {
        let entry = write_of(destination)?;
        let action = format!("{}:{}", entry.namespace, entry.action);
        withholds_call(&iam.lock(), role_arn, &action, destination)
            .filter(|withheld| *withheld)
            .map(|_| provided_role_refusal_message(entry.action, entry.service))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The empty string is the one `DestinationArn` value that is no resource, so it is
    /// held as "no destination"; every other value the member admits is held as it stands.
    #[test]
    fn the_empty_destination_arn_names_no_destination() {
        // (case, the slot's value, what is held for it)
        let rows: [(&str, Option<&str>, Option<&str>); 4] = [
            ("no slot at all", None, None),
            ("the empty string the pattern admits", Some(""), None),
            (
                "a queue",
                Some("arn:aws:sqs:us-east-1:000000000000:records"),
                Some("arn:aws:sqs:us-east-1:000000000000:records"),
            ),
            (
                "an event bus",
                Some("arn:aws:events:us-east-1:000000000000:event-bus/records"),
                Some("arn:aws:events:us-east-1:000000000000:event-bus/records"),
            ),
        ];
        for (case, value, expected) in rows {
            let held = named_destination(value);
            assert_eq!(
                held.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {held:?}"
            );
        }
    }

    /// Each entry is read under the vendor field of the ARNs it is about, and no two
    /// entries claim the same vendor: one kind of destination takes one write.
    #[test]
    fn each_kind_of_destination_states_one_write() {
        for entry in DESTINATION_WRITES {
            let same = DESTINATION_WRITES
                .iter()
                .filter(|other| other.vendor == entry.vendor)
                .count();
            assert_eq!(
                same, 1,
                "case {:?}: expected one entry for the vendor, got {same}",
                entry.vendor
            );
            assert!(
                !entry.action.is_empty() && !entry.service.is_empty(),
                "case {:?}: an entry naming no call refuses nothing legibly",
                entry.vendor
            );
        }
    }

    /// A destination the catalog states a write for is decided on that write; one it
    /// states none for is evidence of nothing.
    #[test]
    fn the_write_a_destination_takes_is_the_one_its_service_names() {
        // (the destination, the IAM action its write is named by)
        let rows: [(&str, Option<&str>); 5] = [
            (
                "arn:aws:sqs:us-east-1:000000000000:records",
                Some("sqs:SendMessage"),
            ),
            (
                "arn:aws:sns:us-east-1:000000000000:records",
                Some("sns:Publish"),
            ),
            (
                "arn:aws:events:us-east-1:000000000000:event-bus/records",
                Some("events:PutEvents"),
            ),
            (
                "arn:aws:lambda:us-east-1:000000000000:function:records",
                Some("lambda:InvokeFunction"),
            ),
            // A bucket's write is an object-level call, so no write is stated for it.
            ("arn:aws:s3:::records", None),
        ];
        for (destination, expected) in rows {
            let stated = write_of(destination)
                .map(|entry| format!("{}:{}", entry.namespace, entry.action));
            assert_eq!(
                stated.as_deref(),
                expected,
                "case {destination:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }
}
