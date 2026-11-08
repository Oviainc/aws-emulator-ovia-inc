// SPDX-License-Identifier: CC0-1.0

//! Which `dataAutomationProfileArn` a submission can be run under.
//!
//! RULE. A Data Automation profile is not a resource an account creates: AWS publishes
//! one profile per GEOGRAPHY, named `<geography>.data-automation-v1`, and a submission
//! names the profile it runs under by the ARN of one of those. So an ARN that satisfies
//! the modelled `DataAutomationProfileArn` `@pattern`
//! (`smithy_query(query="operation",operation="InvokeBlueprintOptimizationAsync")`:
//! `^arn:aws(|-cn|-us-gov):bedrock:[a-zA-Z0-9-]*:(aws|[0-9]{12}):data-automation-profile/[a-zA-Z0-9-_.]+$`)
//! but whose resource id names NO published profile resolves to nothing, and the
//! submission naming it is refused with the modelled `ValidationException` (HTTP 400)
//! before anything is minted.
//!
//! The pattern admits any `[a-zA-Z0-9-_.]+` as the id, so the pattern walk cannot make
//! this decision: it is the resolution of the id against the published profiles, which is
//! what this module holds.

/// What this service answers for a `dataAutomationProfileArn` that resolves to no
/// published profile.
///
/// FIXED text: it names neither the ARN the request carried nor the member that carried
/// it, exactly as this service's other refusals for a reference it cannot resolve name
/// nothing of the request ([`super::BLUEPRINT_ARN_NOT_FOUND_MESSAGE`]).
pub const INVALID_PROFILE_MESSAGE: &str = "Request has invalid Data Automation Profile Arn";

/// The resource type a Data Automation profile ARN spells, as the modelled
/// `DataAutomationProfileArn` pattern writes it (`…:data-automation-profile/<id>`).
const PROFILE_RESOURCE_TYPE: &str = "data-automation-profile";

/// The suffix every published Data Automation profile id carries: the profile's version,
/// which is `v1` for the profiles AWS publishes today.
///
/// Source: the ids AWS publishes for the Data Automation profiles a submission runs
/// under, one per geography — the form `<geography>.data-automation-v1`, which is also the
/// id the recorded submissions of this service carry.
const PROFILE_ID_SUFFIX: &str = ".data-automation-v1";

/// The geographies AWS publishes a Data Automation profile for, which are the
/// `<geography>` half of a published profile id.
///
/// Source: AWS's Data Automation profile list — one cross-region profile per geography
/// (`us`, `eu`, `apac`), the `us` one being the profile the recorded submissions of this
/// service name. A geography outside this list names no profile AWS publishes, which is
/// exactly the reference this service cannot resolve.
const PROFILE_GEOGRAPHIES: &[&str] = &["us", "eu", "apac"];

/// Whether `profile_arn` names a Data Automation profile AWS publishes.
///
/// The ARN's own fields are the model's business (the `@pattern` walk has already refused
/// an ARN of another partition, service or resource type by the time a handler sees one);
/// what is resolved here is the profile the ARN's resource ID names.
#[must_use]
pub fn resolves(profile_arn: &str) -> bool {
    let Some(parsed) = ::service_common::arn::parse(profile_arn) else {
        return false;
    };
    let Some((resource_type, id)) = parsed.resource.split_once('/') else {
        return false;
    };
    if resource_type != PROFILE_RESOURCE_TYPE {
        return false;
    }
    let Some(geography) = id.strip_suffix(PROFILE_ID_SUFFIX) else {
        return false;
    };
    PROFILE_GEOGRAPHIES.contains(&geography)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: only an ARN whose resource id names a published profile resolves; an id
    /// outside that set does not, however well-formed the ARN around it is.
    ///
    /// The rows span every published geography, both account forms the modelled pattern
    /// admits, the partitions it admits, and the ids that name no profile (a random id, a
    /// geography AWS publishes none for, another version, the bare suffix, an id in the
    /// wrong case).
    #[test]
    fn only_a_published_profile_id_resolves() {
        let rows: [(&str, &str, bool); 12] = [
            (
                "the us profile",
                "arn:aws:bedrock:us-west-2:000000000000:data-automation-profile/us.data-automation-v1",
                true,
            ),
            (
                "the eu profile",
                "arn:aws:bedrock:eu-west-1:000000000000:data-automation-profile/eu.data-automation-v1",
                true,
            ),
            (
                "the apac profile",
                "arn:aws:bedrock:ap-southeast-2:000000000000:data-automation-profile/apac.data-automation-v1",
                true,
            ),
            (
                "an AWS-owned account field",
                "arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1",
                true,
            ),
            (
                "another partition",
                "arn:aws-us-gov:bedrock:us-gov-west-1:000000000000:data-automation-profile/us.data-automation-v1",
                true,
            ),
            (
                "the china partition",
                "arn:aws-cn:bedrock:cn-north-1:000000000000:data-automation-profile/apac.data-automation-v1",
                true,
            ),
            (
                "an id naming no profile",
                "arn:aws:bedrock:us-west-2:000000000000:data-automation-profile/4f2b9c1d8a7e",
                false,
            ),
            (
                "a geography AWS publishes no profile for",
                "arn:aws:bedrock:us-west-2:000000000000:data-automation-profile/mars.data-automation-v1",
                false,
            ),
            (
                "another profile version",
                "arn:aws:bedrock:us-west-2:000000000000:data-automation-profile/us.data-automation-v2",
                false,
            ),
            (
                "the version suffix with no geography",
                "arn:aws:bedrock:us-west-2:000000000000:data-automation-profile/.data-automation-v1",
                false,
            ),
            (
                "a published id in the wrong case",
                "arn:aws:bedrock:us-west-2:000000000000:data-automation-profile/US.data-automation-v1",
                false,
            ),
            (
                "an ARN of another resource type",
                "arn:aws:bedrock:us-west-2:000000000000:blueprint/us.data-automation-v1",
                false,
            ),
        ];
        for (case, arn, expected) in rows {
            let actual = resolves(arn);
            assert_eq!(
                actual, expected,
                "case {case:?} ({arn}): expected {expected}, got {actual}"
            );
        }
    }
}
