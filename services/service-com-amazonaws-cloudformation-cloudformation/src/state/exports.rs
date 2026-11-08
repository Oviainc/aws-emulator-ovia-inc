// SPDX-License-Identifier: CC0-1.0

//! CROSS-STACK EXPORTS: which exported output values are IN USE, and what a stack operation
//! that would withdraw one answers.
//!
//! An exported output value another live stack imports is PINNED. "You can't delete a stack
//! if another stack references one of its outputs", and the same holds for an update: the
//! value every importing stack resolved its `Fn::ImportValue` against has to go on being
//! published, so an operation that would remove such an export — or change the value it
//! publishes — is CANCELLED rather than carried out. The cancellation is not a modelled
//! error on the call: the call is accepted and the stack OPERATION is what does not go
//! through, which is why what this module computes is a `StackStatusReason` and not a
//! refusal of a request.
//!
//! Both halves of the relation are derived from the stacks themselves ([`crate::state`]):
//! an export is what a live stack's output declares an `Export` name for, and an import is
//! what a live stack's deployed template resolved against one ([`StoredStack::imports`]).
//! So an export becomes pinned the moment an importing deployment completes and is free
//! again the moment the last importing stack goes.
//!
//! The same index decides the other rule about an export name: "export names must be unique
//! within a Region", so a deployment that would publish a name another live stack already
//! publishes is FAILED ([`StoredStack::conflicting_export`]) rather than carried out — again
//! a `StackStatusReason` and not a refusal of the request, because the operation is accepted
//! and it is the deployment that cannot be carried through.

use std::collections::BTreeMap;

use super::*;

/// The stack operation a cancellation was asked for, which is what its reason is worded
/// about.
///
/// CloudFormation words the cancellation with the operation's own verb in both halves of
/// the sentence ("Delete canceled. Cannot delete export ...", "Update canceled. Cannot
/// update export ..."), so one wording serves both and neither can drift from the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanceledOperation {
    /// A `DeleteStack` of the stack that publishes the export.
    Delete,
    /// An `UpdateStack` — or a change set executed against that stack — that would stop
    /// publishing the export, or publish a different value under it.
    Update,
}

impl CanceledOperation {
    /// The verb the sentence opens with: the operation, capitalised.
    fn named(self) -> &'static str {
        match self {
            Self::Delete => "Delete",
            Self::Update => "Update",
        }
    }

    /// The same verb inside the sentence, where it is lower-cased.
    fn verb(self) -> &'static str {
        match self {
            Self::Delete => "delete",
            Self::Update => "update",
        }
    }
}

/// The `StackStatusReason` an operation CloudFormation cancelled because it would have
/// withdrawn an exported output value another stack still imports: the operation, the
/// export, and the stack that is still consuming it.
///
/// Fixed text around two variable parts — the export name and the importing stack's name,
/// both taken from this store rather than from any one recorded run — and the sentence ends
/// with its period, as every CloudFormation status reason of this class does.
pub fn canceled_reason(
    operation: CanceledOperation,
    export_name: &str,
    importing_stack_name: &str,
) -> String {
    format!(
        "{} canceled. Cannot {} export {export_name} as it is in use by \
         {importing_stack_name}.",
        operation.named(),
        operation.verb(),
    )
}

/// One live stack that imports an exported output value: how this service addresses it
/// (`stack_id`, so the publishing stack can tell its OWN imports apart from another
/// stack's) and the name the cancellation reason reports it by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportingStack {
    pub stack_id: String,
    pub stack_name: String,
}

/// One live stack that PUBLISHES an exported output value: how this service addresses it
/// (`stack_id`, so a deployment can tell its own export apart from another stack's) and the
/// name the failure reports it by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportingStack {
    pub stack_id: String,
    pub stack_name: String,
}

/// The `StackStatusReason` of a deployment that would publish an export name another live
/// stack of the Region already publishes: the name, and the stack that holds it.
///
/// "Export names must be unique within a Region" per account, so the second stack to declare
/// one cannot be deployed: CloudFormation fails that operation rather than publishing two
/// values under one name, where an `Fn::ImportValue` would resolve to whichever stack got
/// there first. Fixed text around two variable parts — the export name the template declares
/// and the name of the stack already publishing it, both taken from this store.
pub fn already_exported_reason(export_name: &str, exporting_stack_name: &str) -> String {
    format!(
        "Export with name {export_name} is already exported by stack \
         {exporting_stack_name}."
    )
}

/// The exported output values one stack publishes, keyed by export name: an output of this
/// stack that declares an `Export` name and carries a value.
///
/// The same reading [`ComAmazonawsCloudformationCloudformationState::exported_values`] gives
/// the whole Region, over one stack's outputs — used over the outputs IN FORCE and over the
/// ones a deployment in flight would install, which is what says whether an export is being
/// withdrawn.
pub fn published(outputs: &[StoredOutput]) -> BTreeMap<String, String> {
    outputs
        .iter()
        .filter_map(|output| {
            let (name, value) = (output.export_name.as_ref()?, output.value.as_ref()?);
            Some((name.clone(), value.clone()))
        })
        .collect()
}

impl ComAmazonawsCloudformationCloudformationState {
    /// Every live stack of one Region that imports an exported output value, keyed by the
    /// export name it imports.
    ///
    /// The index behind `ListImports` ("lists all stacks that are importing an exported
    /// output value"), read for every export at once: a stack operation has to know which
    /// of the exports it publishes are in use, and resolving one name at a time would walk
    /// the store once per export. In creation order, like
    /// [`ComAmazonawsCloudformationCloudformationState::importing_stacks`], so the importer
    /// a cancellation names is the first that imported.
    pub fn importers_by_export(&self, region: &str) -> BTreeMap<String, Vec<ImportingStack>> {
        let mut importers: BTreeMap<String, Vec<ImportingStack>> = BTreeMap::new();
        for stack in self.live_stacks(region) {
            for export_name in stack.imports.keys() {
                importers
                    .entry(export_name.clone())
                    .or_default()
                    .push(ImportingStack {
                        stack_id: stack.stack_id.clone(),
                        stack_name: stack.stack_name.clone(),
                    });
            }
        }
        importers
    }

    /// Which live stack of one Region publishes each exported output value, keyed by export
    /// name.
    ///
    /// The other side of [`Self::importers_by_export`], and the index the uniqueness of an
    /// export name is decided against ([`StoredStack::conflicting_export`]): an export is
    /// what a live stack's output declares an `Export` name for, so a name enters this index
    /// when the publishing deployment completes and leaves it when that stack goes. In
    /// creation order, so the stack a failure names is the one that published the name
    /// first.
    pub fn exporting_stacks(&self, region: &str) -> BTreeMap<String, ExportingStack> {
        let mut publishers: BTreeMap<String, ExportingStack> = BTreeMap::new();
        for stack in self.live_stacks(region) {
            for export_name in published(&stack.outputs).into_keys() {
                publishers
                    .entry(export_name)
                    .or_insert_with(|| ExportingStack {
                        stack_id: stack.stack_id.clone(),
                        stack_name: stack.stack_name.clone(),
                    });
            }
        }
        publishers
    }
}

impl StoredStack {
    /// The export the DEPLOYMENT IN FLIGHT would withdraw from a stack that still imports
    /// it, as the pair (export name, importing stack name), or `None` when nothing it
    /// publishes is in use that way.
    ///
    /// Withdrawn means either half of what an importing stack resolved stops holding: the
    /// template this operation deploys no longer declares that `Export` name at all, or it
    /// declares it with a DIFFERENT value — an importer that resolved the old value would
    /// silently be reading a value the producer no longer publishes. An export the
    /// operation republishes unchanged is withdrawn from nobody, and neither is one only
    /// THIS stack imports: the pin protects the cross-stack reference another stack holds.
    ///
    /// `importers` is [`ComAmazonawsCloudformationCloudformationState::importers_by_export`]
    /// for this stack's Region, read before the stack was resolved for writing.
    pub fn in_use_export_withdrawn(
        &self,
        importers: &BTreeMap<String, Vec<ImportingStack>>,
    ) -> Option<(String, String)> {
        let deployment = self.pending_deployment.as_ref()?;
        let in_force = published(&self.outputs);
        let deploying = published(&deployment.outputs);
        for (export_name, value) in &in_force {
            if deploying.get(export_name) == Some(value) {
                continue;
            }
            let Some(holders) = importers.get(export_name) else {
                continue;
            };
            let importer = holders
                .iter()
                .find(|importer| importer.stack_id != self.stack_id);
            if let Some(importer) = importer {
                return Some((export_name.clone(), importer.stack_name.clone()));
            }
        }
        None
    }

    /// The export the DEPLOYMENT IN FLIGHT would publish under a name another live stack of
    /// the Region already publishes, as the reason that failure carries, or `None` when every
    /// name it declares is free.
    ///
    /// "Export names must be unique within a Region": the value an `Fn::ImportValue`
    /// resolves is the one the publishing stack holds, so two stacks publishing one name is
    /// a state CloudFormation does not let a deployment reach — it fails the operation
    /// instead, with its OWN diagnostic ([`already_exported_reason`]) rather than a resource's
    /// refusal, because no resource is what went wrong.
    ///
    /// A name only THIS stack publishes is no conflict — a re-deploy of the same template
    /// republishes its own exports — and neither is one published by a stack that is no
    /// longer live, since a deleted stack publishes nothing.
    ///
    /// `publishers` is [`ComAmazonawsCloudformationCloudformationState::exporting_stacks`]
    /// for this stack's Region, read before the stack was resolved for writing. One name is
    /// reported, the first in export-name order, which is the one index this service keeps
    /// them in.
    pub fn conflicting_export(
        &self,
        publishers: &BTreeMap<String, ExportingStack>,
    ) -> Option<String> {
        let deployment = self.pending_deployment.as_ref()?;
        published(&deployment.outputs)
            .into_keys()
            .find_map(|export_name| {
                let holder = publishers.get(&export_name)?;
                (holder.stack_id != self.stack_id)
                    .then(|| already_exported_reason(&export_name, &holder.stack_name))
            })
    }
}
