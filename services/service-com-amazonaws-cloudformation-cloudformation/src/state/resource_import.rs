// SPDX-License-Identifier: CC0-1.0

//! Bringing resources that ALREADY EXIST under a stack's management.
//!
//! ## The rule this module implements
//!
//! Two members of `CreateChangeSet` ask for a resource to be ADOPTED rather than created:
//! `ChangeSetType=IMPORT`, whose `ResourcesToImport` names each resource and the identifier
//! it already exists under, and `ImportExistingResources`, which asks CloudFormation to
//! adopt a resource the plan would otherwise CREATE whenever one already exists under the
//! name the template states ("CloudFormation imports it into the stack instead of creating
//! it"). Either way the resource is reported with the `Import` action and the physical id it
//! already carries ([`import_change`], [`adopt_existing`]), and executing the change set
//! records it as a resource the stack now manages WITHOUT creating anything
//! ([`adopted_resources`]) — nothing is asked of the owning service, because the resource
//! it would be asked for is already there.
//!
//! A name is adoptable only while NO stack manages it: a resource another stack owns is that
//! stack's, so adopting it would take over a resource CloudFormation is already managing
//! elsewhere, and the change set fails instead ([`super::physical_name`]).
//!
//! An IMPORT also holds the template to the one rule CloudFormation states for it: every
//! imported resource must carry a `DeletionPolicy`, so that what happens to a resource the
//! stack did not create is stated before the stack takes it over
//! ([`validate_deletion_policies`]).

use super::change_plan::StoredResourceChange;
use super::early_validation::NamedResource;
use super::template::Template;
use super::{change_action, StoredResource};

/// The resource attribute CloudFormation requires on every imported resource:
/// "each resource to import must have a `DeletionPolicy` attribute in your template".
const DELETION_POLICY: &str = "DeletionPolicy";

/// CloudFormation's refusal for an IMPORT whose template gives an imported resource no
/// `DeletionPolicy`.
///
/// The fixed text is CloudFormation's own; the variable part is the list of logical ids
/// that lack the attribute, in the order `ResourcesToImport` names them.
pub fn deletion_policy_required(logical_ids: &[String]) -> String {
    format!(
        "The following resources to import [{}] must have DeletionPolicy attribute \
         specified in the template.",
        logical_ids.join(", ")
    )
}

/// CloudFormation's refusal for an IMPORT that names no resource to import at all: there is
/// nothing for the change set to adopt, so it is no plan.
///
/// This service's own wording for the condition, in the form its other input refusals take
/// (beside `ResourcesToImport can only be specified when ChangeSetType is IMPORT`, which
/// `CreateChangeSet` already answers the converse request with): there is no recorded
/// sentence for it, and its one job is to name the member the request is wrong about.
pub const RESOURCES_TO_IMPORT_REQUIRED: &str =
    "ResourcesToImport must be specified when ChangeSetType is IMPORT";

/// The refusal for an entry naming a logical id the template does not declare: the
/// identifier would be adopted as a resource nothing describes.
///
/// This service's own wording, as [`RESOURCES_TO_IMPORT_REQUIRED`] is, with the logical id
/// the request named as its one variable part.
pub fn not_in_the_template(logical_resource_id: &str) -> String {
    format!(
        "Resource with logical id {logical_resource_id} is not in the template and cannot be \
         imported"
    )
}

/// `Err(message)` unless `template` declares every resource `resources` names, each with a
/// `DeletionPolicy`.
///
/// The two conditions are reported in the order CloudFormation establishes them: a logical id
/// the template does not declare at all cannot be checked for the attribute.
pub fn validate_deletion_policies(
    template: &Template,
    resources: &[crate::ops::types::ResourceToImport],
) -> Result<(), String> {
    let mut without_the_policy = Vec::new();
    for imported in resources {
        let declared = template
            .resources
            .iter()
            .find(|declared| declared.logical_id == imported.logical_resource_id)
            .ok_or_else(|| not_in_the_template(&imported.logical_resource_id))?;
        if declared.declaration.get(DELETION_POLICY).is_none() {
            without_the_policy.push(imported.logical_resource_id.clone());
        }
    }
    if !without_the_policy.is_empty() {
        return Err(deletion_policy_required(&without_the_policy));
    }
    Ok(())
}

/// The plan an IMPORT change set carries: one `Import` change per resource the request names,
/// in that order, each reporting the physical id the resource already exists under.
///
/// The identifier reported is the one the NAMING pipeline computes for the declaration
/// (`named`, from [`super::early_validation::named_resources`]) — the form the owning service
/// identifies the resource by, which is what every other change reports — and the value the
/// request's own `ResourceIdentifier` carries where the declaration states its name nowhere.
pub fn import_changes(
    resources: &[crate::ops::types::ResourceToImport],
    named: &[NamedResource],
) -> Vec<StoredResourceChange> {
    resources
        .iter()
        .map(|imported| {
            let physical_resource_id = named
                .iter()
                .find(|candidate| candidate.logical_id == imported.logical_resource_id)
                .map(|candidate| candidate.physical_resource_id.clone())
                .or_else(|| {
                    // A declaration that states its name nowhere is identified by what the
                    // REQUEST carries, read in key order so one request reads one way.
                    let mut identifier: Vec<(&String, &String)> =
                        imported.resource_identifier.iter().collect();
                    identifier.sort();
                    identifier
                        .first()
                        .map(|(_, value)| (*value).clone())
                });
            import_change(
                &imported.logical_resource_id,
                &imported.resource_type,
                physical_resource_id,
            )
        })
        .collect()
}

/// The resources an IMPORT names, as the candidates the existence check asks the owning
/// services about: the type and the identifier each `Import` change reports.
///
/// A resource to import must BE there — the plan reports the stack as managing a resource
/// that already exists — so the identifier the request named is read back from the service
/// that owns the type, exactly as a name the TEMPLATE states is
/// ([`super::early_validation::taken_names`]), rather than inferred from anything.
pub fn import_candidates(changes: &[StoredResourceChange]) -> Vec<NamedResource> {
    changes
        .iter()
        .filter(|change| change.action == change_action::IMPORT)
        .filter_map(|change| {
            Some(NamedResource {
                logical_id: change.logical_resource_id.clone(),
                resource_type: change.resource_type.clone(),
                physical_resource_id: change.physical_resource_id.clone()?,
            })
        })
        .collect()
}

/// One `Import` change: the resource the change set adopts, under the identifier it already/// exists with.
///
/// `ResourceChange$PhysicalResourceId` is "the resource's physical ID (resource name)", which
/// an adopted resource HAS — unlike an addition, which has none because it does not exist
/// yet. No `Replacement`, `Scope` or `Details` comes with it: nothing about the resource is
/// changed by adopting it.
pub fn import_change(
    logical_resource_id: &str,
    resource_type: &str,
    physical_resource_id: Option<String>,
) -> StoredResourceChange {
    StoredResourceChange {
        action: change_action::IMPORT.to_string(),
        logical_resource_id: logical_resource_id.to_string(),
        resource_type: resource_type.to_string(),
        physical_resource_id,
        scope: Vec::new(),
        replacement: None,
        details: Vec::new(),
        policy_action: None,
        resource_drift_status: None,
    }
}

/// Reclassify as `Import` every ADDITION of `changes` whose resource already exists under the
/// name the template states (`taken`, from [`super::early_validation::taken_names`]).
///
/// This is what `ImportExistingResources` asks for: "if you have resources that exist outside
/// of CloudFormation, you can import them into your stack as part of the change set" — the
/// plan is otherwise unchanged, so a resource nothing holds a name for is still created.
pub fn adopt_existing(changes: &mut [StoredResourceChange], taken: &[NamedResource]) {
    for change in changes
        .iter_mut()
        .filter(|change| change.action == change_action::ADD)
    {
        let Some(existing) = taken
            .iter()
            .find(|candidate| candidate.logical_id == change.logical_resource_id)
        else {
            continue;
        };
        change.action = change_action::IMPORT.to_string();
        change.physical_resource_id = Some(existing.physical_resource_id.clone());
    }
}

/// The resources an execution ADOPTS, as records of resources the stack already has: one per
/// `Import` change, carrying the declaration's own configuration and the identifier the
/// change reports, marked as existing in the service that owns it.
///
/// This is what an import IS: from the moment the change set is executed the stack MANAGES a
/// resource that already exists. The deployment therefore reconciles against a resource the
/// stack HAS ([`super::provisioning::reconcile`]) rather than one it must create — so nothing
/// is asked of the owning service about it, and importing a resource does not modify it — and
/// the stack goes on recording it, so its own deletion disposes of it under the
/// `DeletionPolicy` the template states.
pub fn adopted_resources(
    changes: &[StoredResourceChange],
    declared: &[StoredResource],
) -> Vec<StoredResource> {
    changes
        .iter()
        .filter(|change| change.action == change_action::IMPORT)
        .filter_map(|change| {
            let declaration = declared
                .iter()
                .find(|candidate| candidate.logical_id == change.logical_resource_id)?;
            Some(StoredResource {
                physical_resource_id: change
                    .physical_resource_id
                    .clone()
                    .unwrap_or_else(|| declaration.physical_resource_id.clone()),
                materialised: true,
                ..declaration.clone()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template_of(body: &str) -> Template {
        super::super::declared_template(body).expect("a well-formed template")
    }

    fn imported(logical_id: &str, name: &str) -> crate::ops::types::ResourceToImport {
        crate::ops::types::ResourceToImport {
            resource_type: "AWS::SSM::Parameter".to_string(),
            logical_resource_id: logical_id.to_string(),
            resource_identifier: [("Name".to_string(), name.to_string())]
                .into_iter()
                .collect(),
        }
    }

    /// RULE: an IMPORT is refused unless every resource it names is declared with a
    /// `DeletionPolicy`, and the refusal lists the logical ids that lack one.
    #[test]
    fn an_import_needs_a_deletion_policy_on_every_imported_resource() {
        let body = r#"{"Resources":{
            "Kept":{"Type":"AWS::SSM::Parameter","DeletionPolicy":"Retain",
                    "Properties":{"Name":"/kept","Type":"String","Value":"1"}},
            "Also":{"Type":"AWS::SSM::Parameter","DeletionPolicy":"Delete",
                    "Properties":{"Name":"/also","Type":"String","Value":"1"}},
            "Bare":{"Type":"AWS::SSM::Parameter",
                    "Properties":{"Name":"/bare","Type":"String","Value":"1"}},
            "Other":{"Type":"AWS::SSM::Parameter",
                     "Properties":{"Name":"/other","Type":"String","Value":"1"}}}}"#;
        let template = template_of(body);
        type Row = (
            &'static str,
            Vec<crate::ops::types::ResourceToImport>,
            Option<&'static str>,
        );
        let rows: Vec<Row> = vec![
            ("one resource that carries Retain", vec![imported("Kept", "/kept")], None),
            (
                "two of them, one carrying Delete",
                vec![imported("Kept", "/kept"), imported("Also", "/also")],
                None,
            ),
            (
                "one resource that carries none",
                vec![imported("Bare", "/bare")],
                Some(
                    "The following resources to import [Bare] must have DeletionPolicy \
                     attribute specified in the template.",
                ),
            ),
            (
                "two that carry none, in the order the request names them",
                vec![imported("Other", "/other"), imported("Bare", "/bare")],
                Some(
                    "The following resources to import [Other, Bare] must have \
                     DeletionPolicy attribute specified in the template.",
                ),
            ),
            (
                "a logical id the template does not declare",
                vec![imported("Absent", "/absent")],
                Some(
                    "Resource with logical id Absent is not in the template and cannot be \
                     imported",
                ),
            ),
        ];
        for (case, resources, expected) in &rows {
            let refusal = validate_deletion_policies(&template, resources).err();
            assert_eq!(
                refusal.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?}, got {refusal:?}"
            );
        }
    }

    /// RULE: an addition whose name already exists is reclassified as an `Import` carrying
    /// that name; every other change is left exactly as the comparison computed it.
    #[test]
    fn an_existing_name_turns_an_addition_into_an_import() {
        let change = |action: &str, logical_id: &str| StoredResourceChange {
            action: action.to_string(),
            logical_resource_id: logical_id.to_string(),
            resource_type: "AWS::SSM::Parameter".to_string(),
            physical_resource_id: None,
            scope: Vec::new(),
            replacement: None,
            details: Vec::new(),
            policy_action: None,
            resource_drift_status: None,
        };
        let taken = vec![NamedResource {
            logical_id: "Adopted".to_string(),
            resource_type: "AWS::SSM::Parameter".to_string(),
            physical_resource_id: "/held".to_string(),
        }];
        let rows: &[(&str, &str, &str, Option<&str>)] = &[
            (
                "an addition of a name that exists",
                change_action::ADD,
                "Adopted",
                Some("/held"),
            ),
            ("an addition of a name nothing holds", change_action::ADD, "Fresh", None),
            ("a modification of the adopted resource", change_action::MODIFY, "Adopted", None),
        ];
        for (case, action, logical_id, adopted) in rows {
            let mut changes = vec![change(action, logical_id)];
            adopt_existing(&mut changes, &taken);
            let expected_action = if adopted.is_some() {
                change_action::IMPORT
            } else {
                *action
            };
            assert_eq!(
                (changes[0].action.as_str(), changes[0].physical_resource_id.as_deref()),
                (expected_action, *adopted),
                "case {case:?}: expected {expected_action:?}/{adopted:?}, got {:?}",
                changes[0]
            );
        }
    }
}
