// SPDX-License-Identifier: CC0-1.0

//! The resource types a stack OPERATION may work with: `ResourceTypes`, "specifies which
//! resource types you can work with".
//!
//! ## The rule this module implements
//!
//! A request that states `ResourceTypes` bounds the deploy it opens to those types, and
//! "if the list of resource types doesn't include a resource that you're creating, the
//! stack creation fails" — so the list is weighed against the template BEFORE anything is
//! provisioned, and a template declaring a type the list does not cover is refused with
//! nothing created. A stated list is the caller's own ceiling on what the operation may
//! bring into the account; a list nothing is held to is no ceiling at all.
//!
//! The syntax a list entry may take is the documented one: `AWS::*` for every resource
//! Amazon Web Services publishes, `Custom::*` for every custom resource,
//! `AWS::<service>::*` for every resource of one service, and a type's own name for one
//! type. So an entry ending in `*` covers every type whose name begins with what stands
//! before it, and any other entry covers exactly the type it names
//! ([`covers`]).
//!
//! ## The pair the input documents
//!
//! "Only one of the `Capabilities` and `ResourceType` parameters can be specified": the two
//! members state two different bounds on one request, so a request stating both states no
//! one bound and is refused for the REQUEST SHAPE
//! ([`BOTH_CAPABILITIES_AND_RESOURCE_TYPES`]) — the rule every operation whose input carries
//! that pair is held to, before either bound is weighed.

use super::template::Template;

/// The entry that covers every type under the prefix before it: "`AWS::*` (for all AWS
/// resources)", "`AWS::<service_name>::*` (for all resources of a particular service)".
const EVERY_TYPE_UNDER: char = '*';

/// The refusal for a request that states BOTH of the members that bound what it may work
/// with, as the service words it: each states a different bound, and a request stating two
/// states no one bound to hold it to.
pub const BOTH_CAPABILITIES_AND_RESOURCE_TYPES: &str =
    "ResourceTypes and Capabilities cannot be specified at the same time.";

/// `Err(message)` for a request that states both `Capabilities` and `ResourceTypes` — the
/// rule every operation whose input carries that pair is held to, in the one place that
/// words it.
///
/// A member stated as an EMPTY list states no bound: "only one of the Capabilities and
/// ResourceType parameters can be specified" is about a request that really asks for both.
pub fn one_bound(capabilities: &[String], resource_types: &[String]) -> Result<(), String> {
    if !capabilities.is_empty() && !resource_types.is_empty() {
        return Err(BOTH_CAPABILITIES_AND_RESOURCE_TYPES.to_string());
    }
    Ok(())
}

/// Whether `allowed` — one entry of a stated `ResourceTypes` — covers `resource_type`.
///
/// An entry ending in `*` covers every type whose name starts with the text before it,
/// which is what the documented `AWS::*`, `Custom::*` and `AWS::<service>::*` forms are;
/// every other entry covers the one type it spells.
pub fn covers(allowed: &str, resource_type: &str) -> bool {
    match allowed.strip_suffix(EVERY_TYPE_UNDER) {
        Some(prefix) => resource_type.starts_with(prefix),
        None => allowed == resource_type,
    }
}

/// The types `template` declares that a stated `ResourceTypes` does not cover, in the order
/// the template reader yielded them and each named once.
///
/// A request stating NO list states no bound — "by default, CloudFormation grants
/// permissions to all resource types" — so an empty list excludes nothing. A resource
/// declared under a `Condition` counts, exactly as it does for the capabilities a template
/// requires: which types a template declares is read off the document, before any condition
/// is evaluated.
pub fn not_allowed(template: &Template, allowed: &[String]) -> Vec<String> {
    if allowed.is_empty() {
        return Vec::new();
    }
    let mut excluded: Vec<String> = Vec::new();
    for resource in &template.resources {
        let covered = allowed
            .iter()
            .any(|entry| covers(entry, &resource.resource_type));
        if covered || excluded.contains(&resource.resource_type) {
            continue;
        }
        excluded.push(resource.resource_type.clone());
    }
    excluded
}

/// The message a request whose template declares a type its own `ResourceTypes` does not
/// cover is refused with: the type the request did not permit itself to work with, and the
/// list it stated instead.
///
/// Built from the request — the type the submitted template declares and the list the
/// request put on its own deploy — rather than from any fixed text. The TYPE is the first
/// one the template declares that the list does not cover, which is the one the author has
/// to act on.
pub fn not_allowed_message(resource_type: &str, allowed: &[String]) -> String {
    format!(
        "Resource type {resource_type} is not allowed by parameter ResourceTypes [{}]",
        allowed.join(", ")
    )
}

/// `Some(message)` for a submitted template declaring a type the stated `ResourceTypes`
/// does not cover; `None` for one every declared type is covered by, and for a request that
/// stated no list at all.
pub fn refusal(template: &Template, allowed: &[String]) -> Option<String> {
    let excluded = not_allowed(template, allowed);
    let first = excluded.first()?;
    Some(not_allowed_message(first, allowed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: an entry covers the type it names, and an entry ending in `*` covers every type
    /// under the prefix before it — the documented forms a list entry may take.
    #[test]
    fn an_entry_covers_the_types_its_documented_form_names() {
        // The forms `CreateStackInput$ResourceTypes` documents: `AWS::*`, `Custom::*`,
        // `AWS::<service_name>::*`, `AWS::<service_name>::<resource>` and a custom type's
        // own name.
        let rows: &[(&str, &str, &str, bool)] = &[
            ("a type named exactly", "AWS::SNS::Topic", "AWS::SNS::Topic", true),
            ("another type of the same service", "AWS::SNS::Topic", "AWS::SNS::Subscription", false),
            ("every resource of a service", "AWS::SNS::*", "AWS::SNS::Subscription", true),
            ("another service under a service wildcard", "AWS::SNS::*", "AWS::SQS::Queue", false),
            ("every published resource", "AWS::*", "AWS::S3::Bucket", true),
            ("a custom resource under the published wildcard", "AWS::*", "Custom::Thing", false),
            ("every custom resource", "Custom::*", "Custom::Thing", true),
            ("a custom resource named exactly", "Custom::Thing", "Custom::Thing", true),
            ("a different custom resource", "Custom::Thing", "Custom::Other", false),
        ];
        for (case, allowed, resource_type, expected) in rows {
            let covered = covers(allowed, resource_type);
            assert_eq!(
                covered, *expected,
                "case {case:?}: expected {expected:?}, got {covered:?}"
            );
        }
    }

    /// RULE: the types reported are the declared ones no entry covers, each once; a request
    /// that stated no list reports none.
    #[test]
    fn the_excluded_types_are_the_ones_no_entry_covers() {
        let body = r#"{"Resources":{
            "Topic":{"Type":"AWS::SNS::Topic"},
            "Queue":{"Type":"AWS::SQS::Queue"},
            "Also":{"Type":"AWS::SQS::Queue"},
            "Bucket":{"Type":"AWS::S3::Bucket","Condition":"Never"}}}"#;
        let template = super::super::declared_template(body).expect("a well-formed template");
        let rows: &[(&str, &[&str], &[&str])] = &[
            ("no list at all", &[], &[]),
            (
                "a list covering one of three types",
                &["AWS::SNS::Topic"],
                &["AWS::SQS::Queue", "AWS::S3::Bucket"],
            ),
            (
                "a list covering every type",
                &["AWS::*"],
                &[],
            ),
            (
                "a list naming each type",
                &["AWS::SNS::Topic", "AWS::SQS::Queue", "AWS::S3::Bucket"],
                &[],
            ),
            (
                "a service wildcard",
                &["AWS::SQS::*", "AWS::SNS::*"],
                &["AWS::S3::Bucket"],
            ),
        ];
        for (case, allowed, expected) in rows {
            let stated: Vec<String> = allowed.iter().map(ToString::to_string).collect();
            let excluded = not_allowed(&template, &stated);
            assert_eq!(
                excluded, *expected,
                "case {case:?}: expected {expected:?}, got {excluded:?}"
            );
        }
    }

    /// RULE: a request may state the capabilities it acknowledges or the resource types it
    /// works with, not both.
    #[test]
    fn a_request_states_one_bound_or_the_other() {
        let rows: &[(&str, &[&str], &[&str], bool)] = &[
            ("neither", &[], &[], true),
            ("capabilities alone", &["CAPABILITY_IAM"], &[], true),
            ("resource types alone", &[], &["AWS::*"], true),
            ("both", &["CAPABILITY_IAM"], &["AWS::*"], false),
        ];
        for (case, capabilities, resource_types, expected) in rows {
            let capabilities: Vec<String> = capabilities.iter().map(ToString::to_string).collect();
            let resource_types: Vec<String> =
                resource_types.iter().map(ToString::to_string).collect();
            let allowed = one_bound(&capabilities, &resource_types).is_ok();
            assert_eq!(
                allowed, *expected,
                "case {case:?}: expected {expected:?}, got {allowed:?}"
            );
        }
    }
}
