// SPDX-License-Identifier: CC0-1.0

//! The tag-refusal slice of this service's state: the ONE sentence `AddTags`, `ListTags`
//! and `DeleteTags` state a `ResourceArn` this account holds no resource under with.
//!
//! The tag operations address ANY SageMaker resource kind by ARN, and SageMaker answers
//! each of them with the refusal of the resource FAMILY the ARN names — the same sentence
//! that family's own reader answers — rather than with one family-agnostic prose. So this
//! slice holds no wording and no vocabulary of its own: it resolves the ARN's
//! resource-kind segment and hands the rendering to the family's existing helper, which
//! keeps the tag path and the family's own operations in step by construction.

use super::*;

/// How a tag operation states the resource `resource_arn` names when this account holds
/// none: the not-found sentence of the FAMILY the ARN's resource-kind segment names,
/// over that ARN.
///
/// `region` is the Region the request signed for, which is what decides whether the ARN
/// addresses a resource of this account at all ([`arn_resource`]) — an ARN of another
/// partition, Region or account names no local resource and no family, so it takes the
/// family-agnostic sentence below. A family addressed by the name inside the ARN rather
/// than by the ARN itself gets that name, so the sentence it renders names the SAME ARN
/// the request sent.
///
/// That fallback — [`TAGGED_RESOURCE_ENTITY`] — is what an ARN this resolution maps to
/// no family takes, and it is honestly a FALLBACK: a kind absent from the match below
/// takes it whether or not its family could word something. Two groups of kinds are
/// deliberately left to it, and the rest are simply not mapped yet.
///
/// * The families whose recorded refusal quotes the identifier AS THE CALLER SENT IT —
///   the hosting resources ([`could_not_find`]), the AI benchmarking family
///   ([`ai_entity_not_found`]), a cluster ([`cluster_not_found`]) and the job families:
///   their own docs state the refusal must not name an identifier the service
///   synthesised, and a tag request carries only an ARN, so rendering one of those
///   sentences here would report a value no caller sent.
/// * The resources whose identity is composite rather than one path segment (a Studio
///   app's five members, a hub content's hub-type-name triple, an image VERSION's
///   number), which an ARN's kind segment alone does not resolve.
///
/// Any other kind becomes exact by being added below with its family's helper.
pub fn tagged_resource_not_found(region: &str, resource_arn: &str) -> String {
    let Some((kind, name)) = tagged_resource_kind(region, resource_arn) else {
        return does_not_exist(TAGGED_RESOURCE_ENTITY, resource_arn);
    };
    match kind {
        // The lineage, pipeline and experiment-trial family: the single-quoted ARN.
        ACTION_KIND => does_not_exist(ACTION_ENTITY, resource_arn),
        ARTIFACT_KIND => does_not_exist(ARTIFACT_ENTITY, resource_arn),
        CONTEXT_KIND => does_not_exist(CONTEXT_ENTITY, resource_arn),
        EXPERIMENT_KIND => does_not_exist(EXPERIMENT_ENTITY, resource_arn),
        TRIAL_KIND => does_not_exist(TRIAL_ENTITY, resource_arn),
        TRIAL_COMPONENT_KIND => does_not_exist(TRIAL_COMPONENT_ENTITY, resource_arn),
        LINEAGE_GROUP_KIND => does_not_exist(LINEAGE_GROUP_ENTITY, resource_arn),
        PIPELINE_ARN_KIND => does_not_exist(PIPELINE_ENTITY, resource_arn),
        // The algorithm and model-package family: the same sentence, ARN unquoted.
        ALGORITHM_ARN_KIND => entity_does_not_exist(ALGORITHM_ENTITY, resource_arn),
        MODEL_PACKAGE_GROUP_ARN_KIND => {
            entity_does_not_exist(MODEL_PACKAGE_GROUP_ENTITY, resource_arn)
        }
        MODEL_PACKAGE_KIND => entity_does_not_exist(MODEL_PACKAGE_ENTITY, resource_arn),
        // The `Cannot find <subject>: <arn>` family.
        CODE_REPOSITORY_ARN_KIND => cannot_find(CODE_REPOSITORY_SUBJECT, resource_arn),
        // The HyperPod task-governance family: `<Entity> with arn <arn> could not be
        // found`.
        CLUSTER_SCHEDULER_CONFIG_KIND => cluster_scheduler_config_not_found(region, name),
        COMPUTE_QUOTA_KIND => compute_quota_not_found(region, name),
        // The families wording a sentence of their own over the ARN.
        MODEL_CARD_KIND => model_card_not_found(region, resource_arn),
        NOTEBOOK_INSTANCE_ARN_KIND => notebook_instance_does_not_exist(resource_arn),
        NOTEBOOK_LIFECYCLE_CONFIG_ARN_KIND => {
            notebook_instance_lifecycle_config_not_found(region, name)
        }
        APP_IMAGE_CONFIG_ARN_KIND => app_image_config_not_found(region, name),
        IMAGE_ARN_KIND => image_does_not_exist(resource_arn),
        // The families whose sentence names the NAME inside the ARN, which is what their
        // own readers reduce an ARN form to ([`name_from_arn_or_name`]).
        HUB_KIND => hub_not_found(name),
        FEATURE_GROUP_ARN_KIND => feature_group_not_found(name),
        _ => does_not_exist(TAGGED_RESOURCE_ENTITY, resource_arn),
    }
}

/// The `<kind>` segment of `arn`, and the resource NAME after it, when `arn` is one
/// [`resource_arn`] mints for this account in `region` — i.e. the resource family it
/// names and the identifier within that family — or `None` when it names anything else.
///
/// A resource's ARN is `…:<kind>/<path>`, and a path may carry further `/` components (an
/// app's five, a model package's version, a model card's export job), so the kind is the
/// segment BEFORE the first one and the name is everything after it. An ARN carrying no
/// `/` at all names a kind with an empty name, which no store holds a record under.
fn tagged_resource_kind<'v>(region: &str, arn: &'v str) -> Option<(&'v str, &'v str)> {
    let resource = arn_resource(arn, region)?;
    Some(resource.split_once('/').unwrap_or((resource, "")))
}
