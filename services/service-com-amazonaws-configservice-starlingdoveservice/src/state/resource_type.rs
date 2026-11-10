// SPDX-License-Identifier: CC0-1.0

//! Which resource type NAMES Config accepts, and how it refuses one that is no resource
//! type at all.
//!
//! Config's own `ResourceType` enumerates the types it RECORDS, and the model declares
//! that set as a closed enum. The set of type names Config can be ASKED about is wider
//! than it: the configuration state of a CloudFormation registry type — a custom
//! resource type, recorded through `PutResourceConfig` — is held under the registry's own
//! type name, which the enum does not list, so a listing of the resources discovered
//! under such a name is a question Config answers (with the resources it holds under that
//! exact name, none when it holds none) rather than one it refuses.
//!
//! So a member naming ONE resource type on a discovered-resource LISTING admits either
//! spelling: a type the model enumerates, or a CloudFormation registry type name. A value
//! that is neither names no resource type in either sense, and is refused with Config's
//! own fixed sentence about a resource type it does not support
//! ([`UNSUPPORTED_RESOURCE_TYPE_MESSAGE`]).
//!
//! The members that take a resource type to RECORD it — the resource types a
//! configuration recorder is told to record — are not this: those name types Config
//! itself observes, so they stay inside the modelled set and are refused outside it at
//! the request boundary.

/// How Config refuses a value that is no resource type it supports.
///
/// Config records the resource types its published catalogue covers and that catalogue
/// GROWS, so the service does not read its current value set back to the caller: the
/// sentence is FIXED and request-independent — it quotes neither the value sent nor the
/// types Config supports — and tells the caller to verify the type it named. Recorded on
/// the resource-type association path and stated for the shape rather than for one
/// operation, so it holds wherever a member carrying a resource type is given a value
/// that names none.
pub const UNSUPPORTED_RESOURCE_TYPE_MESSAGE: &str =
    "The resource type you specified is not supported by AWS Config. Verify the resource \
     type, and try again.";

/// The form of a CloudFormation registry type name, as the CloudFormation API's own model
/// declares it (`smithy_query(query="shape", shape="TypeName",
/// model_path="services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json")`
/// gives this `@pattern` beside `@length(10, 204)`): three `::`-separated segments of
/// between two and sixty-four alphanumerics, optionally followed by the module marker.
///
/// The expression is the published one verbatim, so a registry type name is matched by
/// the rule CloudFormation publishes rather than by a reading of it.
const REGISTRY_TYPE_NAME_PATTERN: &str =
    r"^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$";

/// The declared length of a CloudFormation registry type name, from the same shape as
/// [`REGISTRY_TYPE_NAME_PATTERN`] (`@length(10, 204)`).
const REGISTRY_TYPE_NAME_LENGTH: std::ops::RangeInclusive<usize> = 10..=204;

/// [`REGISTRY_TYPE_NAME_PATTERN`], compiled once for the process.
///
/// `None` where the expression could not be compiled — which cannot happen for a constant
/// of this crate, and is carried rather than asserted so no request can be answered with a
/// panic: an uncompilable pattern then admits the value, the same direction the framework's
/// own `@pattern` check takes ("we never reject a value because of an unparseable model
/// pattern", `aws_smithy_schema_server::constraints`).
static REGISTRY_TYPE_NAME: std::sync::LazyLock<Option<::regex::Regex>> =
    std::sync::LazyLock::new(|| ::regex::Regex::new(REGISTRY_TYPE_NAME_PATTERN).ok());

/// Whether `value` is one of the resource types this service's model enumerates — the
/// types Config itself records.
///
/// Read off the generated value set of `com.amazonaws.configservice#ResourceType` rather
/// than a list written here, so this and the request boundary judge the same set.
pub fn is_recorded_resource_type(value: &str) -> bool {
    crate::ops::types::RESOURCE_TYPE_VALUES.contains(&value)
}

/// Whether `value` is a CloudFormation registry type name — the name a custom resource
/// type is registered, and its configuration state recorded, under.
pub fn is_registry_resource_type_name(value: &str) -> bool {
    REGISTRY_TYPE_NAME_LENGTH.contains(&value.chars().count())
        && REGISTRY_TYPE_NAME
            .as_ref()
            .is_none_or(|pattern| pattern.is_match(value))
}

/// The refusal a discovered-resource listing answers `value` with, or `None` when `value`
/// names a resource type such a listing can be asked about.
///
/// Both spellings are a type name: one the model enumerates, or a CloudFormation registry
/// type name. Everything else — a name with characters no type name carries, an empty
/// value — names no resource type, and earns Config's own fixed sentence.
pub fn unsupported_resource_type_refusal(value: &str) -> Option<&'static str> {
    let named = is_recorded_resource_type(value) || is_registry_resource_type_name(value);
    (!named).then_some(UNSUPPORTED_RESOURCE_TYPE_MESSAGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A listing admits a type the model enumerates and a CloudFormation registry type
    /// name, and refuses a value that is neither — whatever way it misses.
    #[test]
    fn a_listing_admits_either_spelling_of_a_resource_type_name() {
        // Accepted rows differ along WHICH spelling they are: types the model
        // enumerates (`crate::ops::types::RESOURCE_TYPE_VALUES`, from
        // `smithy_query(query="shape", shape="ResourceType")`), a third-party registry
        // type, a registry type under another organization's own namespace, one at the
        // published minimum length, and a module type.
        let enumerated = crate::ops::types::RESOURCE_TYPE_VALUES;
        let accepted: Vec<&str> = [
            "AWS::S3::Bucket",
            "AWS::Config::ResourceCompliance",
            enumerated[0],
            enumerated[enumerated.len() - 1],
            "Sim2Real::FidelityProbe::Widget",
            "Organization::Service::Resource",
            // Ten characters: the published minimum length of a registry type name.
            "Ab::Cd::Ef",
            "My::Name::Space::MODULE",
            // A supported type mis-cased is no type Config records, but it IS a
            // registry type name, so a listing is asked about it and holds nothing.
            "aws::s3::bucket",
        ]
        .to_vec();

        for value in accepted {
            let refusal = unsupported_resource_type_refusal(value);
            assert!(
                refusal.is_none(),
                "case {value:?}: a resource type name is admitted, got {refusal:?}"
            );
        }

        // Refused rows differ along HOW the value is no type name at all: characters no
        // segment carries, too few and too many segments, an empty segment, the empty
        // value, and one segment shorter than the published minimum.
        for value in [
            "not-a-resource-type",
            "AWS::S3",
            "AWS::S3::Bucket::Extra",
            "AWS::::Bucket",
            "",
            "A::B::C",
            "AWS::S3::Bucket ",
        ] {
            assert_eq!(
                unsupported_resource_type_refusal(value),
                Some(UNSUPPORTED_RESOURCE_TYPE_MESSAGE),
                "case {value:?}: a value that names no resource type is refused with \
                 Config's own sentence"
            );
        }
    }

    /// The two spellings are distinct questions: the modelled set is what Config
    /// RECORDS, and a registry type name is a name, so neither test subsumes the other.
    #[test]
    fn a_registry_type_name_is_not_a_type_config_records() {
        for value in ["Sim2Real::FidelityProbe::Widget", "aws::s3::bucket"] {
            assert!(
                is_registry_resource_type_name(value),
                "case {value:?}: expected a registry type name"
            );
            assert!(
                !is_recorded_resource_type(value),
                "case {value:?}: expected no type Config records"
            );
        }
        for value in ["AWS::S3::Bucket", "AWS::EC2::Volume"] {
            assert!(
                is_recorded_resource_type(value),
                "case {value:?}: expected a type Config records"
            );
        }
    }
}
