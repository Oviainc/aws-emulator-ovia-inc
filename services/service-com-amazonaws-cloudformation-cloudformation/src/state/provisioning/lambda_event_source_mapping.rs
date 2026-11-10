// SPDX-License-Identifier: CC0-1.0

//! `AWS::Lambda::EventSourceMapping`: the mapping a stack declares, created in AWS Lambda
//! itself — so the trigger a template wires up really reads the event source and really
//! invokes the function.
//!
//! This is the per-type half of provisioning for one resource type, the half
//! [`super::super::provisioner`] holds for every other type this emulator creates. It lives
//! in its own module beside the algorithm that drives it for the reason
//! [`super::lambda_function`] records, and is reached through the same seam
//! ([`super::Owner`]).
//!
//! The type's properties are `CreateEventSourceMapping`'s own members, named identically on
//! both sides, with two exceptions, each the call that owns it:
//!
//! * `KmsKeyArn` is spelled `KMSKeyArn` by the API, exactly as it is on a function;
//! * `Tags` is a LIST of key/value pairs in a template and a MAP on the call, and the set a
//!   resource ends up carrying is the declared one plus the tags CloudFormation propagates
//!   ([`tags_of`]).
//!
//! "Ref returns the event source mapping's ID" — a UUID AWS LAMBDA assigns, which no naming
//! rule could have computed, so the id the create answers is this resource's physical id:
//! that is what `GetEventSourceMapping`, `UpdateEventSourceMapping` and
//! `DeleteEventSourceMapping` address the mapping by, and what the stack publishes for it.
//!
//! `CreateEventSourceMapping` answers a mapping whose `State` is `Creating` while Lambda
//! stands the poller up, and the mapping EXISTS from that moment — every call that addresses
//! one takes it — so the resource's creation is not held open on it ([`readiness`]), exactly
//! as a function's `Pending` window does not hold a function's.

use std::time::Duration;

use serde_json::{Map, Value};

use super::{call_rest_json, percent_encoded, Created, Readiness, Refusal, Removal};
use crate::state::provisioner::tags_of;
use crate::state::StoredResource;

/// The resource type this module provisions.
pub(super) const LAMBDA_EVENT_SOURCE_MAPPING: &str = "AWS::Lambda::EventSourceMapping";

/// The SigV4 signing name of the service that owns it, which is what routes the in-process
/// call to AWS Lambda's own handlers.
const LAMBDA_SIGNING_NAME: &str = "lambda";

/// `CreateEventSourceMapping`'s own URI: `POST /2015-03-31/event-source-mappings`.
const MAPPINGS_URI: &str = "/2015-03-31/event-source-mappings";

/// The error code Lambda answers for a mapping it does not hold (`ResourceNotFoundException`,
/// httpError 404), which is what says a delete has nothing left to remove and a read has no
/// mapping to report.
const MAPPING_NOT_FOUND: &str = "ResourceNotFoundException";

/// The error code Lambda refuses a call about a mapping that is not available for it with
/// (`ResourceInUseException`, httpError 400): "you tried to update an event source mapping in
/// the CREATING state, or you tried to delete an event source mapping currently UPDATING".
const RESOURCE_IN_USE: &str = "ResourceInUseException";

/// The `State` member Lambda reports a mapping's lifecycle in, read by [`removal`] as the
/// state a mapping is still being removed in.
const STATE_MEMBER: &str = "State";

/// The member the create answers the id Lambda assigned the mapping under.
const UUID_MEMBER: &str = "UUID";

/// The property naming the function the mapping invokes, which `CreateEventSourceMapping`
/// requires.
const FUNCTION_NAME_PROPERTY: &str = "FunctionName";

/// The property CloudFormation spells differently from the API member it is
/// (`KmsKeyArn` / `KMSKeyArn`), as on a function.
const KMS_KEY_PROPERTY: &str = "KmsKeyArn";
const KMS_KEY_MEMBER: &str = "KMSKeyArn";

/// The property whose declared pairs become the call's `Tags` map.
const TAGS_PROPERTY: &str = "Tags";

/// The tag-key namespace Lambda reserves, compared in lower case: a tag write carrying one is
/// refused outright, whoever makes it — so the three records CloudFormation keeps about a
/// resource it provisioned stay in CloudFormation's own records.
const RESERVED_TAG_PREFIX: &str = "aws:";

/// How long [`await_settled`] waits between asking Lambda whether the mapping has left the
/// transitional state a further call on it is refused in, and how many times it asks.
///
/// Sized exactly as [`super::lambda_function`]'s own wait: short enough that a settled
/// mapping is noticed about as soon as it settles, and long enough to cover the window Lambda
/// holds one open for on a loaded machine. A wait that runs out is not a failure of its own —
/// what fails then is the call that needed the mapping available, reported as Lambda's own
/// refusal of it.
const SETTLE_CHECK_INTERVAL: Duration = Duration::from_millis(25);
const SETTLE_CHECKS: usize = 80;

/// Every property `AWS::Lambda::EventSourceMapping` publishes. A declared property this list
/// does not carry is REFUSED rather than dropped, so a trigger is never reported as created
/// with less than its declaration asked for.
///
/// Source: the type's own property set, which is `CreateEventSourceMapping`'s member list
/// (`smithy_query(query="operation",operation="CreateEventSourceMapping")`) with the
/// `KmsKeyArn` spelling CloudFormation publishes.
const MAPPING_PROPERTIES: &[&str] = &[
    "AmazonManagedKafkaEventSourceConfig",
    "BatchSize",
    "BisectBatchOnFunctionError",
    "DestinationConfig",
    "DocumentDBEventSourceConfig",
    "Enabled",
    "EventSourceArn",
    "FilterCriteria",
    "FunctionName",
    "FunctionResponseTypes",
    "KmsKeyArn",
    "LoggingConfig",
    "MaximumBatchingWindowInSeconds",
    "MaximumRecordAgeInSeconds",
    "MaximumRetryAttempts",
    "MetricsConfig",
    "ParallelizationFactor",
    "ProvisionedPollerConfig",
    "Queues",
    "ScalingConfig",
    "SelfManagedEventSource",
    "SelfManagedKafkaEventSourceConfig",
    "SourceAccessConfigurations",
    "StartingPosition",
    "StartingPositionTimestamp",
    "Tags",
    "Topics",
    "TumblingWindowInSeconds",
];

/// The properties that ARE `CreateEventSourceMapping`'s own members under the same name, so
/// the declared value travels into the request as written.
const CREATE_MEMBERS: &[&str] = &[
    "AmazonManagedKafkaEventSourceConfig",
    "BatchSize",
    "BisectBatchOnFunctionError",
    "DestinationConfig",
    "DocumentDBEventSourceConfig",
    "Enabled",
    "EventSourceArn",
    "FilterCriteria",
    "FunctionResponseTypes",
    "LoggingConfig",
    "MaximumBatchingWindowInSeconds",
    "MaximumRecordAgeInSeconds",
    "MaximumRetryAttempts",
    "MetricsConfig",
    "ParallelizationFactor",
    "ProvisionedPollerConfig",
    "Queues",
    "ScalingConfig",
    "SelfManagedEventSource",
    "SelfManagedKafkaEventSourceConfig",
    "SourceAccessConfigurations",
    "StartingPosition",
    "StartingPositionTimestamp",
    "Topics",
    "TumblingWindowInSeconds",
];

/// The properties `UpdateEventSourceMapping` takes under the same name — what a change to a
/// mapping that already exists can be applied through.
///
/// Source: that operation's own member list
/// (`smithy_query(query="operation",operation="UpdateEventSourceMapping")`) less the `UUID`
/// that addresses the call. Every other property of the type is create-only: the call takes
/// no member for it, so AWS replaces the mapping rather than editing it ([`update`]).
const UPDATE_MEMBERS: &[&str] = &[
    "AmazonManagedKafkaEventSourceConfig",
    "BatchSize",
    "BisectBatchOnFunctionError",
    "DestinationConfig",
    "DocumentDBEventSourceConfig",
    "Enabled",
    "FilterCriteria",
    "FunctionName",
    "FunctionResponseTypes",
    "LoggingConfig",
    "MaximumBatchingWindowInSeconds",
    "MaximumRecordAgeInSeconds",
    "MaximumRetryAttempts",
    "MetricsConfig",
    "ParallelizationFactor",
    "ProvisionedPollerConfig",
    "ScalingConfig",
    "SourceAccessConfigurations",
    "TumblingWindowInSeconds",
];

/// Create the mapping an `AWS::Lambda::EventSourceMapping` declares, in AWS Lambda itself —
/// so the function the stack deployed really consumes the event source the template named,
/// and a client that calls `lambda get-event-source-mapping` with the id the stack published
/// reads the mapping back.
///
/// `Err` is Lambda's own refusal of the create, which is what the resource fails with: an
/// event source it cannot poll, a function it does not hold, a mapping that would read records
/// the function is already mapped to, or a function execution role the policies in force on it
/// do not let read the named source. The resource is never passed off as created on a call
/// that did not answer 2xx, so a stack whose trigger Lambda refused rolls back instead of
/// reporting a trigger that could never fire.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource)?;
    let mut request = Map::new();
    request.insert(
        FUNCTION_NAME_PROPERTY.to_string(),
        Value::String(function_of(&resource.properties)?),
    );
    for member in CREATE_MEMBERS {
        if let Some(value) = declared_in(&resource.properties, member) {
            request.insert((*member).to_string(), value.clone());
        }
    }
    if let Some(value) = declared_in(&resource.properties, KMS_KEY_PROPERTY) {
        request.insert(KMS_KEY_MEMBER.to_string(), value.clone());
    }
    let tags = storable_tags(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert(
            TAGS_PROPERTY.to_string(),
            Value::Object(
                tags.into_iter()
                    .map(|(key, value)| (key, Value::String(value)))
                    .collect(),
            ),
        );
    }
    let answered = call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "POST",
        MAPPINGS_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    // The id Lambda assigned, which is what `Ref` returns for the type and what every later
    // call about this mapping addresses it by. A create that answered none leaves the resource
    // addressable by nothing, so it is refused rather than recorded.
    let uuid = reported_text(&answered, UUID_MEMBER).ok_or_else(|| Refusal {
        code: "InvalidRequest".to_string(),
        message: format!(
            "AWS Lambda reported no {UUID_MEMBER} for the mapping declared by \
             {LAMBDA_EVENT_SOURCE_MAPPING}, which its own calls address a mapping by"
        ),
    })?;
    Ok(Created {
        physical_resource_id: Some(uuid),
        attributes: ::std::collections::BTreeMap::new(),
    })
}

/// Whether Lambda reports the mapping this create made ready.
///
/// Nothing is asked, and nothing waits: the mapping EXISTS from the moment
/// `CreateEventSourceMapping` answers — `GetEventSourceMapping` resolves it, the update and
/// delete calls take it — and the `Creating` state it answers with is AWS Lambda's own
/// transition while it stands the poller up, not a resource that has yet to come into being.
/// It is the case [`super::lambda_function::readiness`] states for a function's `Pending`.
pub(super) fn readiness() -> Readiness {
    Readiness::Ready
}

/// Apply a CHANGED declaration to the mapping the stack already has, through Lambda's own
/// `UpdateEventSourceMapping` — the call a client makes for the same change.
///
/// Only the properties that call takes can change on a mapping that goes on existing
/// ([`UPDATE_MEMBERS`]); every other property of the type is create-only, so a declaration
/// that changes one of those asks for a DIFFERENT mapping and is reported as not applied
/// rather than as a success that dropped the caller's change.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let asked = StoredResource {
        properties: declared.clone(),
        ..resource.clone()
    };
    refuse_unmapped_properties(&asked)?;
    if create_only_change(&resource.properties, declared) {
        return Ok(None);
    }
    let mut request = Map::new();
    for member in UPDATE_MEMBERS {
        if let Some(value) = declared_in(declared, member) {
            request.insert((*member).to_string(), value.clone());
        }
    }
    if let Some(value) = declared_in(declared, KMS_KEY_PROPERTY) {
        request.insert(KMS_KEY_MEMBER.to_string(), value.clone());
    }
    // Lambda refuses a change to a mapping that is still in a transitional state, so the call
    // is made once that service reports the mapping out of one.
    await_settled(ctx, region, &resource.physical_resource_id).await;
    call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "PUT",
        &mapping_uri(&resource.physical_resource_id),
        Some(&Value::Object(request)),
    )
    .await?;
    // The mapping keeps the id Lambda assigned it, and publishes no attribute this read could
    // add: the change is in force, which is all the deployment needs.
    Ok(Some(Created::default()))
}

/// Delete the mapping a stack created, addressed by the id Lambda assigned it — this
/// resource's physical id, which is what `Ref` returns for the type.
///
/// A mapping Lambda no longer holds is already in the state the delete asks for, judged on the
/// CODE and never on a message. A mapping Lambda is still standing up, or still applying a
/// change to, is one it refuses to delete for now, so the delete waits for it to settle and is
/// then made — which is what a rollback owes a mapping it has just created.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let uri = mapping_uri(&resource.physical_resource_id);
    for attempt in 0..SETTLE_CHECKS {
        match call_rest_json(ctx, LAMBDA_SIGNING_NAME, region, "DELETE", &uri, None).await {
            Ok(_) => return Ok(()),
            Err(refusal) if refusal.code == MAPPING_NOT_FOUND => return Ok(()),
            Err(refusal) if refusal.code == RESOURCE_IN_USE && attempt + 1 < SETTLE_CHECKS => {
                ::tokio::time::sleep(SETTLE_CHECK_INTERVAL).await;
            }
            Err(refusal) => return Err(refusal),
        }
    }
    Ok(())
}

/// Whether Lambda has finished removing the mapping a delete was accepted for.
///
/// "When you delete an event source mapping, it enters a Deleting state and might not be
/// completely deleted for several seconds", so the read is what says the removal is through
/// rather than an assumption about it.
pub(super) async fn removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Removal, Refusal> {
    match call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "GET",
        &mapping_uri(physical_resource_id),
        None,
    )
    .await
    {
        Ok(answered) => Ok(Removal::Pending(
            reported_text(&answered, STATE_MEMBER).unwrap_or_default(),
        )),
        Err(refusal) if refusal.code == MAPPING_NOT_FOUND => Ok(Removal::Gone),
        Err(refusal) => Err(refusal),
    }
}

/// The mapping's configuration as Lambda reports it, for drift detection: the members
/// `GetEventSourceMapping` answers under the same names the type publishes as properties. A
/// mapping that service no longer holds was deleted.
pub(super) async fn actual_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    match call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "GET",
        &mapping_uri(physical_resource_id),
        None,
    )
    .await
    {
        Ok(answered) => {
            let mut reported = Map::new();
            for property in MAPPING_PROPERTIES {
                // The two properties `GetEventSourceMapping` does not answer under the name
                // the type publishes: the function is reported as its ARN under `FunctionArn`,
                // and the tag set is read by a call of its own.
                if *property == FUNCTION_NAME_PROPERTY || *property == TAGS_PROPERTY {
                    continue;
                }
                let member = if *property == KMS_KEY_PROPERTY {
                    KMS_KEY_MEMBER
                } else {
                    property
                };
                if let Some(value) = answered.get(member).filter(|value| !value.is_null()) {
                    reported.insert((*property).to_string(), value.clone());
                }
            }
            Ok(Some(Value::Object(reported)))
        }
        Err(refusal) if refusal.code == MAPPING_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Wait until Lambda reports the mapping out of a TRANSITIONAL state — the window it refuses a
/// further call on the mapping inside ([`RESOURCE_IN_USE`]).
///
/// Returns as soon as the mapping is settled, and gives up after [`SETTLE_CHECKS`] checks. A
/// refusal of the READ ends the wait the same way: it says nothing about whether the mapping is
/// available, and the call that follows is what reports Lambda's answer.
async fn await_settled(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) {
    for _ in 0..SETTLE_CHECKS {
        let Ok(answered) = call_rest_json(
            ctx,
            LAMBDA_SIGNING_NAME,
            region,
            "GET",
            &mapping_uri(physical_resource_id),
            None,
        )
        .await
        else {
            return;
        };
        let settling = reported_text(&answered, STATE_MEMBER)
            .is_some_and(|state| TRANSITIONAL_STATES.contains(&state.as_str()));
        if !settling {
            return;
        }
        ::tokio::time::sleep(SETTLE_CHECK_INTERVAL).await;
    }
}

/// The `State` values a mapping is not available for a further call in, from
/// `ResourceInUseException`'s own sentence: "you tried to update an event source mapping in the
/// CREATING state, or you tried to delete an event source mapping currently UPDATING".
const TRANSITIONAL_STATES: &[&str] = &["Creating", "Updating"];

/// Whether this deployment changes a property that can only be set when the mapping is
/// CREATED: a property `UpdateEventSourceMapping` takes no member for, which is why AWS
/// replaces the mapping for such a change rather than editing it.
fn create_only_change(existing: &Value, declared: &Value) -> bool {
    MAPPING_PROPERTIES
        .iter()
        .filter(|property| {
            !UPDATE_MEMBERS.contains(*property) && **property != KMS_KEY_PROPERTY
        })
        .any(|property| declared_in(declared, property) != declared_in(existing, property))
}

/// The function the mapping invokes, as the declaration names it: a name, a partial ARN or a
/// full ARN, each of which `CreateEventSourceMapping`'s `FunctionName` admits. The member is
/// required, so a declaration naming no function is refused rather than wired to nothing.
fn function_of(properties: &Value) -> Result<String, Refusal> {
    match declared_in(properties, FUNCTION_NAME_PROPERTY).and_then(Value::as_str) {
        Some(function) if !function.is_empty() => Ok(function.to_string()),
        _ => Err(Refusal {
            code: "InvalidRequest".to_string(),
            message: format!(
                "{LAMBDA_EVENT_SOURCE_MAPPING} requires the {FUNCTION_NAME_PROPERTY} property"
            ),
        }),
    }
}

/// The tags of a declared mapping that Lambda will STORE: the ones the declaration asks for
/// and the ones CloudFormation propagates ([`tags_of`]), less every key in the namespace
/// Lambda reserves that is not CloudFormation's own — the same rule a function's tags are
/// held to ([`super::lambda_function`]), because it is the same service's tag-writing rule.
fn storable_tags(properties: &Value, propagated: &[(String, String)]) -> Vec<(String, String)> {
    tags_of(properties, propagated)
        .into_iter()
        .filter(|(key, _)| {
            super::is_stack_provenance_tag(key)
                || !key.to_ascii_lowercase().starts_with(RESERVED_TAG_PREFIX)
        })
        .collect()
}

/// The URI that addresses ONE mapping: the `@httpLabel` every mapping operation binds the id
/// to, percent-encoded as a non-greedy label is sent.
fn mapping_uri(uuid: &str) -> String {
    format!("{MAPPINGS_URI}/{}", percent_encoded(uuid))
}

/// One declared property, as the template evaluated it.
fn declared_in<'a>(properties: &'a Value, name: &str) -> Option<&'a Value> {
    properties.get(name).filter(|value| !value.is_null())
}

/// One member of an answer read as TEXT: the id and the state Lambda reports.
fn reported_text(answered: &Value, member: &str) -> Option<String> {
    answered
        .get(member)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was: refusing is what keeps a trigger from being reported as created with less than its
/// declaration asked for.
fn refuse_unmapped_properties(resource: &StoredResource) -> Result<(), Refusal> {
    let Value::Object(properties) = &resource.properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if MAPPING_PROPERTIES.contains(&property.as_str()) {
            continue;
        }
        return Err(Refusal {
            code: "NotImplemented".to_string(),
            message: format!(
                "this emulator does not apply the {property} property of {}",
                resource.resource_type
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every member the create sends is a published property of the type, and every published
    /// property is either a create member or one of the three this module maps itself — so no
    /// declared property can be silently dropped.
    #[test]
    fn every_published_property_has_a_place() {
        for property in MAPPING_PROPERTIES {
            let placed = CREATE_MEMBERS.contains(property)
                || [FUNCTION_NAME_PROPERTY, KMS_KEY_PROPERTY, TAGS_PROPERTY].contains(property);
            assert!(
                placed,
                "case {property:?}: expected the property to be a create member or one of the \
                 three the arm maps itself, got neither"
            );
        }
        for member in CREATE_MEMBERS {
            assert!(
                MAPPING_PROPERTIES.contains(member),
                "case {member:?}: expected the create member to be a published property of \
                 {LAMBDA_EVENT_SOURCE_MAPPING}, got none"
            );
        }
        for member in UPDATE_MEMBERS {
            assert!(
                MAPPING_PROPERTIES.contains(member),
                "case {member:?}: expected the update member to be a published property of \
                 {LAMBDA_EVENT_SOURCE_MAPPING}, got none"
            );
        }
    }

    /// A change to a property the update call takes is applied in place; a change to one it
    /// takes no member for is a change only a replacement could make.
    #[test]
    fn only_a_change_the_update_call_cannot_take_is_create_only() {
        // (case, the declaration before, the declaration after, whether it is create-only)
        let rows: &[(&str, Value, Value, bool)] = &[
            (
                "an unchanged declaration",
                ::serde_json::json!({ "EventSourceArn": "arn:aws:sqs:us-east-1:000000000000:q", "BatchSize": 1 }),
                ::serde_json::json!({ "EventSourceArn": "arn:aws:sqs:us-east-1:000000000000:q", "BatchSize": 1 }),
                false,
            ),
            (
                "a changed batch size, which the update call takes",
                ::serde_json::json!({ "EventSourceArn": "arn:aws:sqs:us-east-1:000000000000:q", "BatchSize": 1 }),
                ::serde_json::json!({ "EventSourceArn": "arn:aws:sqs:us-east-1:000000000000:q", "BatchSize": 5 }),
                false,
            ),
            (
                "a mapping paused, which the update call takes",
                ::serde_json::json!({ "EventSourceArn": "arn:aws:sqs:us-east-1:000000000000:q", "Enabled": true }),
                ::serde_json::json!({ "EventSourceArn": "arn:aws:sqs:us-east-1:000000000000:q", "Enabled": false }),
                false,
            ),
            (
                "another event source, which it does not",
                ::serde_json::json!({ "EventSourceArn": "arn:aws:sqs:us-east-1:000000000000:q" }),
                ::serde_json::json!({ "EventSourceArn": "arn:aws:sqs:us-east-1:000000000000:other" }),
                true,
            ),
            (
                "a starting position, which it does not",
                ::serde_json::json!({ "EventSourceArn": "arn:aws:kinesis:us-east-1:000000000000:stream/s" }),
                ::serde_json::json!({ "EventSourceArn": "arn:aws:kinesis:us-east-1:000000000000:stream/s", "StartingPosition": "LATEST" }),
                true,
            ),
        ];
        for (case, existing, declared, expected) in rows {
            let create_only = create_only_change(existing, declared);
            assert_eq!(
                create_only, *expected,
                "case {case:?}: expected create_only_change to be {expected}, got {create_only}"
            );
        }
    }

    /// A declaration naming no function is refused rather than a mapping being wired to
    /// nothing, and one naming it in any of the forms the member admits is taken as written.
    #[test]
    fn the_function_a_declaration_names_is_required() {
        // (case, the declared properties, the function the call carries)
        let rows: &[(&str, Value, Option<&str>)] = &[
            ("no properties at all", Value::Null, None),
            (
                "properties without the function",
                ::serde_json::json!({ "BatchSize": 1 }),
                None,
            ),
            (
                "an empty function name",
                ::serde_json::json!({ "FunctionName": "" }),
                None,
            ),
            (
                "a function name",
                ::serde_json::json!({ "FunctionName": "pump" }),
                Some("pump"),
            ),
            (
                "a function ARN",
                ::serde_json::json!({ "FunctionName": "arn:aws:lambda:us-east-1:000000000000:function:pump" }),
                Some("arn:aws:lambda:us-east-1:000000000000:function:pump"),
            ),
        ];
        for (case, properties, expected) in rows {
            let named = function_of(properties);
            match expected {
                Some(function) => assert_eq!(
                    named.as_deref().ok(),
                    Some(*function),
                    "case {case:?}: expected the call to carry {function:?}, got {named:?}"
                ),
                None => assert!(
                    named.is_err(),
                    "case {case:?}: expected a refusal naming the required property, got {named:?}"
                ),
            }
        }
    }
}
