// SPDX-License-Identifier: CC0-1.0

//! What a client READS ABOUT A TEMPLATE before it deploys one: the capabilities the
//! template's own declarations require, the resource types it defines, and the properties an
//! import of a resource of each of those types takes.
//!
//! Two operations answer with exactly this reading — `ValidateTemplate` ("validates a
//! specified template") and `GetTemplateSummary` ("returns information about a new or
//! existing template … useful for viewing parameter information … before you create or
//! update a stack or StackSet") — so it lives in ONE place: a client that validates a
//! template and then summarises it cannot be told two different things about the same body,
//! and neither answer can drift from what the operations that SUBMIT the body hold it to.
//!
//! Nothing here reads or writes stored state: every answer is a function of the template
//! document alone.

use ::std::collections::{BTreeMap, BTreeSet};

use super::template::{self, RequiredCapabilities, Template};

/// Whether `body` INVOKES a macro, so a request submitting it has to acknowledge
/// `CAPABILITY_AUTO_EXPAND`.
///
/// A template invokes one in either of the two documented ways: its `Transform` section
/// naming a macro for the whole template, or an `Fn::Transform` intrinsic naming one for part
/// of it. Both hand the template to processing that may rewrite it, which is what the
/// acknowledgement is about — "if your stack template contains one or more macros, and you
/// choose to create a stack directly from the processed template […] you must acknowledge
/// this capability".
///
/// The ONE reading of that question: the capabilities a template REPORTS
/// ([`required_capabilities`]) and the acknowledgement a submitting operation REQUIRES
/// ([`super::missing_auto_expand_capability`]) are the same rule, so a template that reports
/// the capability is a template the create refuses without it.
pub fn requires_auto_expand(body: &str) -> bool {
    !template::body_declared_transforms(body).is_empty() || super::include::body_invokes_a_macro(body)
}

/// "The capabilities found within the template": every `Capability` value a request
/// submitting this template has to acknowledge, and the resources that generated them.
///
/// Two independent rules decide the set, and a template subject to both reports both:
///
/// * its RESOURCES require the IAM acknowledgement — `CAPABILITY_IAM`, or
///   `CAPABILITY_NAMED_IAM` where the template gives one of those resources a custom name
///   ([`template::required_capabilities`]);
/// * the MACROS it invokes require `CAPABILITY_AUTO_EXPAND` ([`requires_auto_expand`]).
///
/// `CapabilitiesReason` is "the list of resources that generated the values in the
/// Capabilities response element", so it names the IAM resources and nothing else: a macro is
/// not a resource of the template, and the reason states what a reader can look up in the
/// `Resources` section.
pub fn required_capabilities(body: &str, declared: &Template) -> RequiredCapabilities {
    let mut required = template::required_capabilities(declared);
    if requires_auto_expand(body) {
        required
            .capabilities
            .push(super::CAPABILITY_AUTO_EXPAND.to_string());
    }
    required
}

/// "A list of all the template resource types that are defined in the template": the SET of
/// types the template's resources have, each once.
///
/// A set has no order of its own, and the one CloudFormation reports is not the order the
/// template happens to declare its resources in — a template declaring a bucket before a role
/// is summarised with the role first — so the types are reported sorted by name. That is one
/// answer per template body, however the body spells its `Resources` section and in whatever
/// order a reader walked it.
pub fn resource_types(declared: &Template) -> Vec<String> {
    let types: BTreeSet<String> = template::declared_resource_types(declared)
        .into_iter()
        .collect();
    types.into_iter().collect()
}

/// One `ResourceIdentifierSummary`: the resources of ONE type the template declares, and the
/// properties an import of one of them takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentifierSummary {
    /// `ResourceType`: "the template resource type of the target resources, such as
    /// AWS::S3::Bucket."
    pub resource_type: String,
    /// `LogicalResourceIds`: "the logical IDs of the target resources of the specified
    /// ResourceType, as defined in the import template."
    pub logical_resource_ids: Vec<String>,
    /// `ResourceIdentifiers`: "the resource properties you can provide during the import to
    /// identify your target resources" — this type's primary identifier
    /// ([`super::primary_identifier`]).
    pub resource_identifiers: Vec<String>,
}

/// "A list of resource identifier summaries that describe the target resources of an import
/// operation and the properties you can provide during the import to identify the target
/// resources": one entry per resource TYPE the template declares, carrying the logical ids
/// declared with that type and the properties that identify a resource of it.
///
/// A summary describes the resources an import would ADDRESS, so it is ordered by the logical
/// ids it names: each entry's ids in order, and the entries by the first id of each type. That
/// is one answer per template body — the order a reader happened to walk the `Resources`
/// section in is not part of it, and this crate's reader walks a JSON object in whichever
/// order the build's `serde_json` gives it.
pub fn identifier_summaries(declared: &Template) -> Vec<IdentifierSummary> {
    let mut logical_ids: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for resource in &declared.resources {
        logical_ids
            .entry(resource.resource_type.as_str())
            .or_default()
            .insert(resource.logical_id.as_str());
    }
    let mut summaries: Vec<IdentifierSummary> = logical_ids
        .into_iter()
        .map(|(resource_type, ids)| IdentifierSummary {
            resource_identifiers: super::primary_identifier::properties(resource_type),
            resource_type: resource_type.to_string(),
            logical_resource_ids: ids.into_iter().map(str::to_string).collect(),
        })
        .collect();
    summaries.sort_by(|one, other| {
        one.logical_resource_ids
            .first()
            .cmp(&other.logical_resource_ids.first())
    });
    summaries
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A template declaring a macro reports the acknowledgement that macro requires,
    /// whichever way it invokes one — and a template invoking none reports no capability.
    #[test]
    fn the_capabilities_reported_are_the_ones_a_submitting_request_must_acknowledge() {
        let rows: &[(&str, &str, &[&str])] = &[
            (
                "a Transform section naming a macro",
                r#"{"Transform":"MyMacro","Resources":{"Data":{"Type":"AWS::S3::Bucket"}}}"#,
                &["CAPABILITY_AUTO_EXPAND"],
            ),
            (
                "an Fn::Transform inside a resource",
                r#"{"Resources":{"Data":{"Type":"AWS::S3::Bucket","Properties":{
                     "Fn::Transform":{"Name":"AWS::Include","Parameters":{
                       "Location":"s3://b/s.json"}}}}}}"#,
                &["CAPABILITY_AUTO_EXPAND"],
            ),
            (
                "a macro and an IAM resource",
                r#"{"Transform":"MyMacro","Resources":{"Role":{"Type":"AWS::IAM::Role"}}}"#,
                &["CAPABILITY_IAM", "CAPABILITY_AUTO_EXPAND"],
            ),
            (
                "an IAM resource with a custom name and no macro",
                r#"{"Resources":{"Role":{"Type":"AWS::IAM::Role","Properties":{
                     "RoleName":"named"}}}}"#,
                &["CAPABILITY_NAMED_IAM"],
            ),
            (
                "neither a macro nor an IAM resource",
                r#"{"Resources":{"Data":{"Type":"AWS::S3::Bucket"}}}"#,
                &[],
            ),
        ];
        for (case, body, expected) in rows {
            let declared = template::parse(body)
                .unwrap_or_else(|| panic!("case {case:?}: expected a readable template"));
            let reported = required_capabilities(body, &declared).capabilities;
            assert_eq!(
                reported,
                expected
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect::<Vec<String>>(),
                "case {case:?}: expected {expected:?}, got {reported:?}"
            );
        }
    }

    /// The types a template defines are reported as a SET, sorted by name rather than in the
    /// order the body declares its resources.
    #[test]
    fn the_resource_types_are_the_sorted_set_of_the_types_declared() {
        let body = r#"{"Resources":{
            "Zebra":{"Type":"AWS::S3::Bucket"},
            "Alpha":{"Type":"AWS::IAM::Role"},
            "Beta":{"Type":"AWS::S3::Bucket"}}}"#;
        let declared = template::parse(body).expect("a readable template");
        let reported = resource_types(&declared);
        assert_eq!(
            reported,
            vec!["AWS::IAM::Role".to_string(), "AWS::S3::Bucket".to_string()],
            "case \"a bucket declared before a role\": expected the sorted set, got \
             {reported:?}"
        );
    }

    /// One summary per type, carrying every logical id declared with it and that type's
    /// primary identifier, ordered by the logical ids they name — whichever order the build's
    /// reader walked the `Resources` section in.
    #[test]
    fn one_identifier_summary_describes_each_type_the_template_declares() {
        let body = r#"{"Resources":{
            "Data":{"Type":"AWS::S3::Bucket"},
            "Archive":{"Type":"AWS::S3::Bucket"},
            "Notify":{"Type":"AWS::SNS::Topic"}}}"#;
        let declared = template::parse(body).expect("a readable template");
        let reported = identifier_summaries(&declared);
        assert_eq!(
            reported,
            vec![
                IdentifierSummary {
                    resource_type: "AWS::S3::Bucket".to_string(),
                    logical_resource_ids: vec!["Archive".to_string(), "Data".to_string()],
                    resource_identifiers: vec!["BucketName".to_string()],
                },
                IdentifierSummary {
                    resource_type: "AWS::SNS::Topic".to_string(),
                    logical_resource_ids: vec!["Notify".to_string()],
                    resource_identifiers: vec!["TopicArn".to_string()],
                },
            ],
            "case \"two buckets and a topic\": got {reported:?}"
        );
    }
}
