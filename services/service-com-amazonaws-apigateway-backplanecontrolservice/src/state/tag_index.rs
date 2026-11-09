// SPDX-License-Identifier: CC0-1.0

//! What a CROSS-SERVICE reader of this account's tagged resources needs from this
//! service: which ARNs of the shared `apigateway` namespace are this service's to
//! resolve, and every stored record of it that carries tags, under the ARN this service
//! reports for it.
//!
//! Both answers are derived from the very records and the very ARN minter this service's
//! own `GetTags`, `TagResource` and `UntagResource` resolve through
//! ([`super::parse_tagged_resource`], [`super::tags_of`], [`super::tagged_resource_arn`]),
//! so there is no second copy of a tag or of an ARN form to keep in step: a record
//! created with tags is in the next read of this projection, a record deleted through
//! this service is gone from it, and nothing here needs retracting.
//!
//! A child module of [`super`] rather than more of that file: this is the one surface
//! another crate reads, and keeping it in its own file leaves the state module itself to
//! the records and the operations of this service.

use std::collections::BTreeMap;

use super::{
    ARN_SERVICE, ComAmazonawsApigatewayBackplanecontrolserviceState, TaggedResource,
    tagged_resource_arn,
};

/// Whether `resource_arn` names a resource of THIS service: an ARN of this service's
/// vendor namespace whose leading resource-portion segment is one this service's own
/// operations are bound under.
///
/// What [`super::parse_tagged_resource`] answers `None` for divides in two, and a caller
/// needs to tell them apart: an ARN of a kind this service addresses but does not tag is
/// a bad request TO THIS SERVICE, while an ARN of the shared `apigateway` namespace
/// naming another API Gateway service's resource (API Gateway V2's `/apis/{apiId}`) is
/// not this service's at all.
///
/// WHICH segments are this service's comes from the generated, model-derived
/// [`crate::ROUTE_PATH_SEGMENTS`] — the same list `crate::service` claims requests by —
/// so this answer cannot drift from the operations the model actually binds, and a
/// resource portion is this service's exactly when a URI of this service could be.
pub fn addresses_own_resource(resource_arn: &str) -> bool {
    let Some(arn) = ::service_common::arn::parse_of(resource_arn, ARN_SERVICE) else {
        return false;
    };
    let Some(path) = arn.resource.strip_prefix('/') else {
        return false;
    };
    let leading = path.split('/').next().unwrap_or_default();
    crate::ROUTE_PATH_SEGMENTS.contains(&leading)
}

/// Every stored resource of this service that CARRIES a tag, as `(ARN, tags)` in
/// `region`.
///
/// The records [`super::ComAmazonawsApigatewayBackplanecontrolserviceState::tags_of`]
/// resolves an ARN to, enumerated the other way round: each row's ARN is
/// [`tagged_resource_arn`]'s, so a tag operation of this service resolves it back to that
/// same record, and each row's tags are a snapshot of the record's OWN map, taken under
/// the lock this call holds.
///
/// A record holding NO tag is left out rather than reported with an empty map: there is
/// nothing to project for it, and a reader of tagged resources discards it anyway.
pub fn tagged_resources(
    state: &ComAmazonawsApigatewayBackplanecontrolserviceState,
    region: &str,
) -> Vec<(String, BTreeMap<String, String>)> {
    let mut rows: Vec<(TaggedResource, &BTreeMap<String, String>)> = Vec::new();
    for api in state.rest_apis.values() {
        rows.push((
            TaggedResource::RestApi {
                rest_api_id: api.id.clone(),
            },
            &api.tags,
        ));
        for stage in api.stages.values() {
            rows.push((
                TaggedResource::Stage {
                    rest_api_id: api.id.clone(),
                    stage_name: stage.stage_name.clone(),
                },
                &stage.tags,
            ));
        }
    }
    for key in state.api_keys.values() {
        rows.push((
            TaggedResource::ApiKey {
                api_key_id: key.id.clone(),
            },
            &key.tags,
        ));
    }
    for link in state.vpc_links.values() {
        rows.push((
            TaggedResource::VpcLink {
                vpc_link_id: link.id.clone(),
            },
            &link.tags,
        ));
    }
    for plan in state.usage_plans.values() {
        rows.push((
            TaggedResource::UsagePlan {
                usage_plan_id: plan.id.clone(),
            },
            &plan.tags,
        ));
    }
    for domain in state.domain_names.values() {
        rows.push((
            TaggedResource::DomainName {
                domain_name_key: domain.key(),
            },
            &domain.tags,
        ));
    }
    for association in state.domain_name_access_associations.values() {
        rows.push((
            TaggedResource::DomainNameAccessAssociation {
                access_association_id: association.id.clone(),
            },
            &association.tags,
        ));
    }
    for certificate in state.client_certificates.values() {
        rows.push((
            TaggedResource::ClientCertificate {
                client_certificate_id: certificate.id.clone(),
            },
            &certificate.tags,
        ));
    }
    rows.into_iter()
        .filter(|(_, tags)| !tags.is_empty())
        .map(|(resource, tags)| (tagged_resource_arn(region, &resource), tags.clone()))
        .collect()
}
