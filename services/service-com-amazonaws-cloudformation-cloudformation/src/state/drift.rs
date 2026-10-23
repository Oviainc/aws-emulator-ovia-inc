// SPDX-License-Identifier: CC0-1.0

//! Detecting whether a stack's resources have DRIFTED from the configuration its template
//! declared.
//!
//! ## The rule this module implements
//!
//! `DetectStackDrift` "compares the actual configuration of the resource with its expected
//! template configuration" and "only resource properties explicitly defined in the stack
//! template are checked for drift"; a stack "is considered to have drifted if one or more
//! of its resources differ from their expected template configurations".
//!
//! So the comparison has two sides, and each comes from a different place:
//!
//! * the EXPECTED configuration is [`StoredResource::properties`] — the resource's declared
//!   `Properties` with the template's intrinsic functions evaluated, which is exactly the
//!   request the owning service's create was given;
//! * the ACTUAL configuration is what the OWNING SERVICE reports for the resource now
//!   ([`Provisioner::actual_configuration`](super::provisioner::Provisioner::actual_configuration)),
//!   read through the same in-process, signed callback the create went out on.
//!
//! Nothing here reports a resource as checked that was not read from its owning service:
//! a resource of a type this emulator provisions nothing for, or whose owning service does
//! not serve the read, is `NOT_CHECKED` — "any resources that do not currently support
//! drift detection have a status of NOT_CHECKED".
//!
//! ## What each status means here
//!
//! * `DELETED` — the owning service answers that it no longer holds the resource.
//! * `MODIFIED` — a property the template declares differs from the value that service
//!   reports under the same name.
//! * `IN_SYNC` — the resource is there and no declared property it reports differs.
//! * `NOT_CHECKED` — nothing was asked of any service for it (declared-only resource, a
//!   type with no provisioner, or an owning service whose read is still a scaffold stub).
//! * `UNKNOWN` — the read was refused for another reason, so the check could not run; the
//!   detection as a whole is then `DETECTION_FAILED`, with that refusal as its reason.
//!
//! Only the properties BOTH sides name are compared: a service whose read spells the same
//! configuration under other member names reports nothing comparable, and a resource of
//! such a type is reported as existing rather than as modified on a name mismatch.

use serde_json::Value;

use super::provisioner::provisioner_for;
use super::{resource_drift_status, StoredResource, DRIFT_STATUS_DRIFTED, DRIFT_STATUS_IN_SYNC, DRIFT_STATUS_UNKNOWN};

/// The error code an owning service answers when the read this module needs is still one
/// of this emulator's scaffold stubs. Such a resource cannot be checked for drift here,
/// which is what `NOT_CHECKED` states.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// One resource's drift outcome, as the detection computed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDrift {
    pub logical_id: String,
    /// One of [`resource_drift_status`].
    pub status: String,
}

/// What one drift detection found across the resources it covered.
#[derive(Debug, Clone, Default)]
pub struct Detected {
    /// One entry per resource the detection covered, in the order they were checked.
    pub results: Vec<ResourceDrift>,
    /// The reason the detection could not be carried out for at least one resource — an
    /// owning service's own refusal. `None` when every covered resource was either checked
    /// or is one no check is supported for.
    pub failure: Option<String>,
}

impl Detected {
    /// The stack-level `StackDriftStatus` these per-resource outcomes add up to.
    ///
    /// `DRIFTED` when one or more resources differ from their expected configuration ("a
    /// stack is considered to have drifted if one or more of its resources have drifted"),
    /// `UNKNOWN` when drift detection could not be run for a resource of the stack, and
    /// `IN_SYNC` otherwise — including for a stack none of whose resources support drift
    /// detection, which has no resource differing from its template.
    pub fn stack_drift_status(&self) -> &'static str {
        if self.drifted_count() > 0 {
            return DRIFT_STATUS_DRIFTED;
        }
        if self
            .results
            .iter()
            .any(|result| result.status == resource_drift_status::UNKNOWN)
        {
            return DRIFT_STATUS_UNKNOWN;
        }
        DRIFT_STATUS_IN_SYNC
    }

    /// How many resources differ from their expected configuration —
    /// `DriftedStackResourceCount`, which "will be 0 for stacks whose drift status is
    /// IN_SYNC".
    pub fn drifted_count(&self) -> i32 {
        let drifted = self
            .results
            .iter()
            .filter(|result| is_drifted(&result.status))
            .count();
        i32::try_from(drifted).unwrap_or(i32::MAX)
    }
}

/// Whether a resource-level drift status is one that makes the resource — and so its
/// stack — drifted.
pub fn is_drifted(status: &str) -> bool {
    status == resource_drift_status::MODIFIED || status == resource_drift_status::DELETED
}

/// Run one drift detection over `resources`, asking each resource's owning service what
/// that resource IS now.
///
/// The calls are made with no store lock held (the caller is what releases it), one
/// resource at a time in the order the stack holds them, so a detection costs one read per
/// covered resource and no more.
pub async fn detect(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resources: &[StoredResource],
) -> Detected {
    let mut detected = Detected::default();
    for resource in resources {
        let status = match provisioner_for(&resource.resource_type) {
            // Nothing was created for this resource in any service, so there is no actual
            // configuration to compare: the type does not support drift detection here.
            Some(provisioner) if resource.materialised => {
                match provisioner.actual_configuration(ctx, region, resource).await {
                    Ok(None) => resource_drift_status::DELETED.to_string(),
                    Ok(Some(actual)) => {
                        if differs(&resource.properties, &actual) {
                            resource_drift_status::MODIFIED.to_string()
                        } else {
                            resource_drift_status::IN_SYNC.to_string()
                        }
                    }
                    // The owning service does not serve the read, so this resource's
                    // configuration cannot be checked here.
                    Err(refusal) if refusal.code == NOT_IMPLEMENTED => {
                        resource_drift_status::NOT_CHECKED.to_string()
                    }
                    Err(refusal) => {
                        // "DETECTION_FAILED: The stack drift detection operation has
                        // failed for at least one resource in the stack."
                        detected.failure.get_or_insert_with(|| {
                            format!(
                                "Failed to detect drift on resource {}: {refusal}",
                                resource.logical_id
                            )
                        });
                        resource_drift_status::UNKNOWN.to_string()
                    }
                }
            }
            _ => resource_drift_status::NOT_CHECKED.to_string(),
        };
        detected.results.push(ResourceDrift {
            logical_id: resource.logical_id.clone(),
            status,
        });
    }
    detected
}

/// Whether the ACTUAL configuration differs from the EXPECTED one, over the properties the
/// template declared.
///
/// Only a declared property the actual document also carries under the same name is
/// compared — "only resource properties explicitly defined in the stack template are
/// checked for drift", and a property the owning service's read does not report is not
/// evidence of anything. A scalar is compared on its TEXT, because a configuration a
/// template writes as a number or a boolean is carried as the string an API member takes
/// (`VisibilityTimeout: 30` against `"30"`); anything else is compared as the document it
/// is.
fn differs(expected: &Value, actual: &Value) -> bool {
    let (Some(expected), Some(actual)) = (expected.as_object(), actual.as_object()) else {
        return false;
    };
    expected.iter().any(|(property, declared)| {
        actual
            .get(property)
            .is_some_and(|reported| !same_value(declared, reported))
    })
}

/// Whether two configuration values are the same value.
///
/// A scalar is compared on its text. A COMPOSITE the service reports as a STRING is the
/// document the create sent it as (an `AWS::SQS::Queue`'s `RedrivePolicy` is declared as
/// an object and carried as the JSON text of an attribute), so the reported text is PARSED
/// and compared as the document it is — key order and spacing are the serialiser's
/// choice, not a change to the configuration — and only a text that is not JSON at all
/// falls back to comparing the text.
fn same_value(expected: &Value, actual: &Value) -> bool {
    match (scalar_text(expected), scalar_text(actual)) {
        (Some(expected), Some(actual)) => expected == actual,
        (None, Some(reported)) => match ::serde_json::from_str::<Value>(&reported) {
            Ok(document) => *expected == document,
            Err(_) => {
                // Not a document at all: the only comparison left is the text the create
                // would have sent, which is this value's own compact JSON.
                let sent = expected.to_string();
                sent == reported
            }
        },
        _ => expected == actual,
    }
}

/// One JSON scalar as its single textual spelling, or `None` for a composite.
///
/// The one spelling rule the comparison needs, shared with the provisioning side
/// ([`crate::state::scalar_text`]) so the two cannot disagree about how a scalar a
/// template wrote as a number or a boolean reaches an API member.
fn scalar_text(value: &Value) -> Option<String> {
    crate::state::scalar_text(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A declared property the owning service reports with the same value is not drift,
    /// whichever spelling each side carries it in; one it reports differently is.
    #[test]
    fn only_a_declared_property_the_service_reports_differently_is_drift() {
        for (case, expected, actual, drifted) in [
            (
                "same text",
                serde_json::json!({"VisibilityTimeout": "30"}),
                serde_json::json!({"VisibilityTimeout": "30"}),
                false,
            ),
            (
                "number against the text an attribute carries",
                serde_json::json!({"VisibilityTimeout": 30}),
                serde_json::json!({"VisibilityTimeout": "30"}),
                false,
            ),
            (
                "boolean against its text",
                serde_json::json!({"FifoQueue": true}),
                serde_json::json!({"FifoQueue": "true"}),
                false,
            ),
            (
                "changed value",
                serde_json::json!({"VisibilityTimeout": 30}),
                serde_json::json!({"VisibilityTimeout": "60"}),
                true,
            ),
            (
                "property the read does not report",
                serde_json::json!({"DelaySeconds": 5}),
                serde_json::json!({"VisibilityTimeout": "30"}),
                false,
            ),
            (
                "nested document unchanged",
                serde_json::json!({"VersioningConfiguration": {"Status": "Enabled"}}),
                serde_json::json!({"VersioningConfiguration": {"Status": "Enabled"}}),
                false,
            ),
            (
                "nested document changed",
                serde_json::json!({"VersioningConfiguration": {"Status": "Enabled"}}),
                serde_json::json!({"VersioningConfiguration": {"Status": "Suspended"}}),
                true,
            ),
            (
                "a declared document the service reports as the JSON text it was sent as",
                serde_json::json!({"RedrivePolicy": {"maxReceiveCount": 3}}),
                serde_json::json!({"RedrivePolicy": "{\"maxReceiveCount\":3}"}),
                false,
            ),
            (
                "the same document the service re-serialised its own way",
                serde_json::json!({"RedrivePolicy": {
                    "deadLetterTargetArn": "arn:aws:sqs:us-east-1:000000000000:dlq",
                    "maxReceiveCount": 3
                }}),
                serde_json::json!({"RedrivePolicy": "{ \"maxReceiveCount\": 3, \
                    \"deadLetterTargetArn\": \"arn:aws:sqs:us-east-1:000000000000:dlq\" }"}),
                false,
            ),
            (
                "a declared document the service reports changed",
                serde_json::json!({"RedrivePolicy": {"maxReceiveCount": 3}}),
                serde_json::json!({"RedrivePolicy": "{\"maxReceiveCount\":9}"}),
                true,
            ),
            (
                "resource that declared no properties",
                Value::Null,
                serde_json::json!({"Status": "ACTIVE"}),
                false,
            ),
        ] {
            let actual_drift = differs(&expected, &actual);
            assert_eq!(
                actual_drift, drifted,
                "case {case:?}: expected drifted={drifted}, got {actual_drift}"
            );
        }
    }

    /// The stack-level status is what the per-resource outcomes add up to.
    #[test]
    fn stack_status_follows_the_resource_outcomes() {
        for (case, statuses, expected_status, expected_count) in [
            (
                "nothing checked",
                vec![resource_drift_status::NOT_CHECKED],
                DRIFT_STATUS_IN_SYNC,
                0,
            ),
            (
                "all in sync",
                vec![resource_drift_status::IN_SYNC, resource_drift_status::IN_SYNC],
                DRIFT_STATUS_IN_SYNC,
                0,
            ),
            (
                "one modified",
                vec![resource_drift_status::IN_SYNC, resource_drift_status::MODIFIED],
                DRIFT_STATUS_DRIFTED,
                1,
            ),
            (
                "one deleted and one modified",
                vec![
                    resource_drift_status::DELETED,
                    resource_drift_status::MODIFIED,
                ],
                DRIFT_STATUS_DRIFTED,
                2,
            ),
            (
                "a read that could not run",
                vec![resource_drift_status::UNKNOWN, resource_drift_status::IN_SYNC],
                DRIFT_STATUS_UNKNOWN,
                0,
            ),
            (
                "a drifted resource outweighs one that could not be read",
                vec![
                    resource_drift_status::UNKNOWN,
                    resource_drift_status::MODIFIED,
                ],
                DRIFT_STATUS_DRIFTED,
                1,
            ),
        ] {
            let detected = Detected {
                results: statuses
                    .iter()
                    .enumerate()
                    .map(|(index, status)| ResourceDrift {
                        logical_id: format!("Resource{index}"),
                        status: (*status).to_string(),
                    })
                    .collect(),
                failure: None,
            };
            let status = detected.stack_drift_status();
            let count = detected.drifted_count();
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected {expected_status}, got {status}"
            );
            assert_eq!(
                count, expected_count,
                "case {case:?}: expected {expected_count} drifted, got {count}"
            );
        }
    }
}
