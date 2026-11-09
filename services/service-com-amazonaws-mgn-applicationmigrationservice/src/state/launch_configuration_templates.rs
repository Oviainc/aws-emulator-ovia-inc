// SPDX-License-Identifier: CC0-1.0

//! The LAUNCH CONFIGURATION TEMPLATE concern of this service's state: the templates an
//! account holds, and the refusals the operations that address one word.
//!
//! A template is addressed by its `launchConfigurationTemplateID`, and the record holds
//! what every template carries whatever its launch configuration says — the id, the ARN
//! and the account it lives in. The configuration members join the record with the
//! operation that supplies them (`CreateLaunchConfigurationTemplate`, still a stub).

use super::*;

/// The most launch configuration templates this store holds at a time — per account AND
/// in total.
///
/// The emulator's own bound on the template growth site, not a claim about any AWS
/// quota: the same reasoning as [`APPLICATION_QUOTA`]. Whatever stores a template
/// checks
/// [`ComAmazonawsMgnApplicationmigrationserviceState::launch_configuration_template_store_is_full`]
/// before minting anything, so the record count cannot be grown past the bound by
/// varying the request's account.
pub const LAUNCH_CONFIGURATION_TEMPLATE_QUOTA: usize = 1_000;

/// One stored launch configuration template.
#[derive(Debug, Clone, Default)]
pub struct LaunchConfigurationTemplate {
    pub launch_configuration_template_id: String,
    pub arn: String,
    /// The account the template lives in, so one account's templates are never
    /// another's to read or delete.
    pub account_id: String,
}

impl ComAmazonawsMgnApplicationmigrationserviceState {
    /// The template `template_id` names within `account_id`, or `None` — a template of
    /// another account is not visible to this one.
    pub fn launch_configuration_template_of(
        &self,
        account_id: &str,
        template_id: &str,
    ) -> Option<&LaunchConfigurationTemplate> {
        self.launch_configuration_templates
            .get(template_id)
            .filter(|template| template.account_id == account_id)
    }

    /// Whether the template store is full: either the account already holds
    /// [`LAUNCH_CONFIGURATION_TEMPLATE_QUOTA`] templates, or the whole store does.
    pub fn launch_configuration_template_store_is_full(&self, account_id: &str) -> bool {
        self.launch_configuration_templates.len() >= LAUNCH_CONFIGURATION_TEMPLATE_QUOTA
            || self
                .launch_configuration_templates
                .values()
                .filter(|template| template.account_id == account_id)
                .count()
                >= LAUNCH_CONFIGURATION_TEMPLATE_QUOTA
    }

    /// Store a launch configuration template, remembering its place in creation order.
    ///
    /// Answers whether it was stored, exactly as [`Self::insert_application`] does: a
    /// store already at [`LAUNCH_CONFIGURATION_TEMPLATE_QUOTA`] refuses rather than
    /// growing past the bound, and re-storing an id the map already holds keeps its
    /// place in the creation order rather than duplicating it.
    pub fn insert_launch_configuration_template(
        &mut self,
        template: LaunchConfigurationTemplate,
    ) -> bool {
        let id = template.launch_configuration_template_id.clone();
        let known = self.launch_configuration_templates.contains_key(&id);
        if !known && self.launch_configuration_templates.len() >= LAUNCH_CONFIGURATION_TEMPLATE_QUOTA
        {
            return false;
        }
        if !known {
            self.launch_configuration_template_order.push(id.clone());
        }
        self.launch_configuration_templates.insert(id, template);
        true
    }

    /// Drop one of `account_id`'s templates from the store and from the creation order,
    /// answering the removed record.
    ///
    /// The account key is REQUIRED, exactly as it is on the read accessor above: the map
    /// is keyed by id alone, so a removal that took only the request-controlled
    /// `launchConfigurationTemplateID` could destroy another account's record. `None`
    /// (nothing removed) when the id names no template of this account.
    pub fn remove_launch_configuration_template(
        &mut self,
        account_id: &str,
        template_id: &str,
    ) -> Option<LaunchConfigurationTemplate> {
        self.launch_configuration_template_of(account_id, template_id)?;
        self.launch_configuration_template_order
            .retain(|id| id != template_id);
        self.launch_configuration_templates.remove(template_id)
    }
}

/// The ARN of a launch configuration template:
/// `arn:aws:mgn:<region>:<account>:launch-configuration-template/<launchConfigurationTemplateID>`.
///
/// Nothing mints one yet: `CreateLaunchConfigurationTemplate` is the operation that
/// supplies a template's ARN and its configuration, and it is still a scaffolded stub
/// (it carries the `SCAFFOLDED, NOT IMPLEMENTED` marker), so the only writers of this
/// store today are the tests. The spelling lives here so the create that lands later
/// words it the way every other kind's ARN in this store is worded.
pub fn launch_configuration_template_arn(region: &str, template_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("launch-configuration-template/{template_id}"),
    )
}

/// The modelled refusal for a `launchConfigurationTemplateID` the account holds no
/// record of.
pub fn launch_configuration_template_not_found(template_id: &str) -> ResourceNotFoundException {
    resource_not_found(LAUNCH_CONFIGURATION_TEMPLATE_RESOURCE_TYPE, template_id)
}

/// The `resourceType` a launch-configuration-template refusal names: the modelled shape
/// `com.amazonaws.mgn#LaunchConfigurationTemplate`, as the API spells the kind.
const LAUNCH_CONFIGURATION_TEMPLATE_RESOURCE_TYPE: &str = "LaunchConfigurationTemplate";
