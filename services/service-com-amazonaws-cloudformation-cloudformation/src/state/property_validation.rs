// SPDX-License-Identifier: CC0-1.0

//! The PRE-DEPLOYMENT validation a stack operation's resources are held to: the property
//! values a resource type's own contract admits.
//!
//! ## The rule this module implements
//!
//! A stack operation runs "pre-deployment validations" unless the request turns them off
//! (`DisableValidation`, "set to true to disable pre-deployment validations in changeset or
//! stack operations", default `false`), and what those validations are FOR is catching a
//! declaration CloudFormation already knows cannot be deployed: the operation then fails
//! before the resource is provisioned, so no resource of the stack is created, changed or
//! asked for at all, and no resource-level event is recorded for it. A request that disables
//! them deploys the declaration as written and the same problem surfaces where the resource
//! is really created — as that resource's own failure, carrying the owning service's
//! refusal.
//!
//! ## What this stage can decide, and what it deliberately cannot
//!
//! CloudFormation validates a declaration against the resource TYPE'S SCHEMA, which its
//! registry holds for every type. This emulator holds no such schema: the one it derives for
//! a published type carries that type's RETURN VALUES and states no writable property, no
//! type and no value set at all ([`super::resource_schema`] says so in full), and the service
//! that owns a type is reachable only by CREATING the resource — there is no read that asks
//! it whether a declaration would be accepted. So this stage is NOT that schema check and
//! does not stand in for it: it decides the one question a published value set makes
//! decidable without asking anybody — a property whose contract publishes the values it
//! admits, carrying a value that is not one of them — over the rows this emulator holds
//! ([`CONSTRAINED_PROPERTIES`]).
//!
//! That makes the stage deliberately PARTIAL, and fail-open: a property no row covers, a type
//! with no row at all, and a value an expression produced that is not text are each left to
//! the service that owns the resource, which is the authority on everything this is not. So
//! it can only ever anticipate a refusal that service would have made, never invent one — and
//! a declaration it passes is still provisioned and still judged there. It becomes the real
//! schema check when this registry holds real schemas, or when an owning service serves a
//! would-this-be-accepted read; until then the rows are what it knows.
//!
//! ## Where the value sets come from
//!
//! [`CONSTRAINED_PROPERTIES`] is a catalogue: one row per (resource type, property) whose
//! admitted values the type's own contract publishes, read off the "AWS resource and
//! property types reference" of the CloudFormation User Guide for that type and the API
//! member the property is passed to. It is deliberately a SET of rows rather than a schema
//! language: what this stage needs is the question "is this value one the property admits",
//! and a row answers exactly that.
//!
//! The set a row carries is the one the OWNING API publishes for the member the property
//! becomes, which is the authority on whether a create carrying it can succeed: a value
//! outside it is one the owning service would refuse, so the validation refuses it first —
//! and a value inside it is never refused here.

use ::serde_json::Value;

use super::StoredResource;

/// One property whose admitted values its type's contract publishes.
#[derive(Debug, Clone, Copy)]
pub struct ConstrainedProperty {
    /// The resource type the property belongs to, as a template declares it.
    pub resource_type: &'static str,
    /// The property's name, as a template spells it.
    pub property: &'static str,
    /// The values the property admits, in the order the contract publishes them.
    pub admitted: &'static [&'static str],
}

/// The properties this stage knows a published value set for.
///
/// Source for each row: the resource type's entry in the "AWS resource and property types
/// reference" of the CloudFormation User Guide, whose property documentation publishes the
/// allowed values, and the API member that property is passed to
/// ([`super::provisioner`] is where each type's mapping onto that member lives).
pub const CONSTRAINED_PROPERTIES: &[ConstrainedProperty] = &[
    // `AWS::SSM::Parameter`'s `Type` is `PutParameter$Type`, "the type of parameter",
    // whose published values are the parameter types Systems Manager holds.
    ConstrainedProperty {
        resource_type: "AWS::SSM::Parameter",
        property: "Type",
        admitted: &["String", "StringList", "SecureString"],
    },
    // ...and its `Tier` is `PutParameter$Tier`, "the parameter tier to assign to a
    // parameter".
    ConstrainedProperty {
        resource_type: "AWS::SSM::Parameter",
        property: "Tier",
        admitted: &["Standard", "Advanced", "Intelligent-Tiering"],
    },
    // `AWS::DynamoDB::Table`'s `BillingMode` is `CreateTable$BillingMode`, "controls how
    // you are charged for read and write throughput".
    ConstrainedProperty {
        resource_type: "AWS::DynamoDB::Table",
        property: "BillingMode",
        admitted: &["PROVISIONED", "PAY_PER_REQUEST"],
    },
    // ...and its `TableClass` is `CreateTable$TableClass`, "the table class of the new
    // table".
    ConstrainedProperty {
        resource_type: "AWS::DynamoDB::Table",
        property: "TableClass",
        admitted: &["STANDARD", "STANDARD_INFREQUENT_ACCESS"],
    },
    // `AWS::S3::Bucket`'s `AccessControl` is the canned ACL `CreateBucket$ACL` takes, "a
    // canned access control list (ACL) that grants predefined permissions to the bucket".
    ConstrainedProperty {
        resource_type: "AWS::S3::Bucket",
        property: "AccessControl",
        admitted: &[
            "AuthenticatedRead",
            "AwsExecRead",
            "BucketOwnerFullControl",
            "BucketOwnerRead",
            "LogDeliveryWrite",
            "Private",
            "PublicRead",
            "PublicReadWrite",
        ],
    },
    // `AWS::StepFunctions::StateMachine`'s `StateMachineType` is
    // `CreateStateMachine$type`, "determines whether a Standard or an Express workflow is
    // created".
    ConstrainedProperty {
        resource_type: "AWS::StepFunctions::StateMachine",
        property: "StateMachineType",
        admitted: &["STANDARD", "EXPRESS"],
    },
    // `AWS::Lambda::Function`'s `PackageType` is `CreateFunction$PackageType`, "the type of
    // deployment package".
    ConstrainedProperty {
        resource_type: "AWS::Lambda::Function",
        property: "PackageType",
        admitted: &["Zip", "Image"],
    },
    // `AWS::EC2::VPC`'s `InstanceTenancy` is `CreateVpc$InstanceTenancy`, "the tenancy
    // options for instances launched into the VPC".
    ConstrainedProperty {
        resource_type: "AWS::EC2::VPC",
        property: "InstanceTenancy",
        admitted: &["default", "dedicated", "host"],
    },
];

/// The name this service reports for the pre-deployment validation that holds a
/// declaration's property values to what its type's contract admits.
///
/// `OperationEvent$ValidationName` is "the name of the validation that was performed", and a
/// validation of this service is named for WHAT it validated, in the form the other one this
/// service performs is named in ([`super::early_validation::NAME_CONFLICT_VALIDATION`], the
/// existence check's own name).
pub const PROPERTY_VALIDATION: &str = "PROPERTY_VALIDATION";

/// What a failed pre-deployment property validation is: the resource it failed on, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyFailure {
    /// The logical id of the resource whose declaration failed the validation.
    pub logical_id: String,
    /// That resource's declared type.
    pub resource_type: String,
    /// Why it failed: which property, and what its type admits ([`failure_reason`]).
    pub reason: String,
}

/// `Some(failure)` for the first resource of `declared` carrying a property value its type's
/// contract does not admit — the failure the operation reports instead of deploying
/// anything; `None` when every value this stage can judge is admitted.
///
/// The resources are walked in the order the deployment would have created them, so the
/// failure reported is the one the operation would have run into first. A value that is not
/// text (an expression this service left unevaluated) is no value of a published set either
/// way and is left to the owning service.
pub fn failure(declared: &[StoredResource]) -> Option<PropertyFailure> {
    declared.iter().find_map(|resource| {
        CONSTRAINED_PROPERTIES
            .iter()
            .filter(|constrained| constrained.resource_type == resource.resource_type)
            .find_map(|constrained| {
                let value = resource
                    .properties
                    .get(constrained.property)
                    .and_then(Value::as_str)?;
                if constrained.admitted.contains(&value) {
                    return None;
                }
                Some(PropertyFailure {
                    logical_id: resource.logical_id.clone(),
                    resource_type: resource.resource_type.clone(),
                    reason: failure_reason(
                        &resource.logical_id,
                        constrained.property,
                        constrained.admitted,
                    ),
                })
            })
    })
}

/// How this stage reports a property whose value its type does not admit: the property it is
/// about, in the path form a resource's property is named in, the resource that declared it,
/// and the values the property admits.
///
/// The sentence is the one this crate already words a property-validation failure with — the
/// `Property validation failure: [Value for property {/<Property>} …]` a resource handler of
/// this service answers for a declaration it cannot carry out
/// ([`super::provisioning::nested_stack`]) — so one kind of failure reads the same whether it
/// was caught before provisioning or reported during it. Its variable parts are the
/// declaration's own: the property, the resource that declared it, and the set that property
/// publishes.
///
/// The value the declaration carried is deliberately NOT quoted back: it may be a value a
/// template author marked `NoEcho`, and what the author has to act on is the set the
/// property admits.
pub fn failure_reason(logical_id: &str, property: &str, admitted: &[&str]) -> String {
    format!(
        "Property validation failure: [Value for property {{/{property}}} of resource \
         [{logical_id}] must be one of [{}]]",
        admitted.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::std::collections::BTreeMap;

    /// One declared resource of `resource_type` carrying `properties`.
    fn declared(logical_id: &str, resource_type: &str, properties: Value) -> StoredResource {
        StoredResource {
            logical_id: logical_id.to_string(),
            resource_type: resource_type.to_string(),
            physical_resource_id: format!("{logical_id}-physical"),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
        }
    }

    /// RULE: a property value outside the set its type's contract publishes fails the
    /// validation, and one inside it does not — over rows of the catalogue the issue did
    /// not cite as well as the one it did.
    #[test]
    fn a_value_outside_the_published_set_fails_the_validation() {
        let rows: &[(&str, &str, &str, Value, bool)] = &[
            (
                "a parameter type the contract does not publish",
                "AWS::SSM::Parameter",
                "Type",
                ::serde_json::json!({"Type": "NotAType", "Value": "x"}),
                false,
            ),
            (
                "each parameter type it does",
                "AWS::SSM::Parameter",
                "Type",
                ::serde_json::json!({"Type": "StringList", "Value": "a,b"}),
                true,
            ),
            (
                "a tier outside the published set",
                "AWS::SSM::Parameter",
                "Tier",
                ::serde_json::json!({"Type": "String", "Tier": "Platinum"}),
                false,
            ),
            (
                "a billing mode outside the published set",
                "AWS::DynamoDB::Table",
                "BillingMode",
                ::serde_json::json!({"BillingMode": "FREE"}),
                false,
            ),
            (
                "a billing mode inside it",
                "AWS::DynamoDB::Table",
                "BillingMode",
                ::serde_json::json!({"BillingMode": "PAY_PER_REQUEST"}),
                true,
            ),
            (
                "a canned ACL outside the published set",
                "AWS::S3::Bucket",
                "AccessControl",
                ::serde_json::json!({"AccessControl": "EverybodyRead"}),
                false,
            ),
            (
                "a workflow type outside the published set",
                "AWS::StepFunctions::StateMachine",
                "StateMachineType",
                ::serde_json::json!({"StateMachineType": "FAST"}),
                false,
            ),
            (
                "a tenancy outside the published set",
                "AWS::EC2::VPC",
                "InstanceTenancy",
                ::serde_json::json!({"InstanceTenancy": "shared"}),
                false,
            ),
            (
                "a property the declaration states as an unevaluated expression",
                "AWS::SSM::Parameter",
                "Type",
                ::serde_json::json!({"Type": {"Ref": "Kind"}}),
                true,
            ),
            (
                "a type this stage knows no value set for",
                "AWS::SNS::Topic",
                "DisplayName",
                ::serde_json::json!({"DisplayName": "anything at all"}),
                true,
            ),
        ];
        for (case, resource_type, property, properties, admitted) in rows {
            let resources = vec![declared("R", resource_type, properties.clone())];
            let failed = failure(&resources);
            assert_eq!(
                failed.is_none(),
                *admitted,
                "case {case:?}: expected admitted={admitted:?}, got {failed:?}"
            );
            if let Some(failure) = failed {
                assert!(
                    failure.reason.contains(&format!("{{/{property}}}"))
                        && failure.reason.contains("[R]")
                        && failure.logical_id == "R",
                    "case {case:?}: expected a failure naming the property and the resource, \
                     got {failure:?}"
                );
            }
        }
    }

    /// RULE: the failure reported is the FIRST one the deployment would have run into.
    #[test]
    fn the_first_failing_resource_is_the_one_reported() {
        let resources = vec![
            declared(
                "Fine",
                "AWS::DynamoDB::Table",
                ::serde_json::json!({"BillingMode": "PROVISIONED"}),
            ),
            declared(
                "First",
                "AWS::SSM::Parameter",
                ::serde_json::json!({"Type": "NotAType"}),
            ),
            declared(
                "Second",
                "AWS::S3::Bucket",
                ::serde_json::json!({"AccessControl": "EverybodyRead"}),
            ),
        ];
        let failed = failure(&resources);
        assert_eq!(
            failed.as_ref().map(|failure| failure.reason.as_str()),
            Some(
                failure_reason("First", "Type", &["String", "StringList", "SecureString"]).as_str()
            ),
            "case \"two failing resources\": expected the first one's reason, got {failed:?}"
        );
    }
}
