// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a parameter whose value its model does not admit.
//!
//! Every such refusal of this service is the one class its model documents for it —
//! `InvalidParameterValueException`, "The value that you provided for the specified
//! parameter is invalid." (`smithy_query(query="shape",shape="InvalidParameterValueException")`:
//! a client error, `@httpError` 400, carrying one `message`) — and one SENTENCE: the
//! parameter the value sat at, as the API addresses it, and the requirement that value had
//! to satisfy. The sentence and every clause in it are the shared statements of them
//! (`service_common::validation_message`), so this module chooses only what is this
//! service's: which constraint classes it states that way, and how it quotes the
//! expression an ARN member's value had to match.
//!
//! ## Which parameter the sentence names
//!
//! The one the REQUEST carried, spelled as the API addresses it: a list element by its
//! zero-based index on the member that holds it
//! (`distributions[0].amiDistributionConfiguration.name`), never by the framework walk's own
//! `distributions.1.member.…`, which names no parameter of this API.
//!
//! ## Which requirement it states
//!
//! The one the value broke, for the constraint classes this service's recorded refusals
//! establish a clause for: a `@pattern`, a `@range` bound, and an enumerated value set. A
//! class they do not — a `@length` bound, a repeated element of a `@uniqueItems` list, a
//! member the shape requires and the request omits — is left to the framework's own
//! refusal rather than being given a clause this service has never been seen to state.
//! One of them, the `@range` MINIMUM, this service words its own way
//! ([`must_be_at_least`]); every other clause is the shared statement of it.
//!
//! ## How it quotes an ARN expression, and why that is per BINDING
//!
//! A refusal of this service quotes the expression an ARN member's value had to match in
//! the form the layer that READ the value writes it, and the two layers spell the partition
//! and Region fields differently: a value read out of the request BODY is refused with the
//! Region-code form (`arn:aws(?:-[a-z]+)*:imagebuilder:[a-z]{2,}(?:-[a-z]+)+-[0-9]+:`),
//! and a value read out of the REQUEST LINE — an `@httpQuery` parameter, an `@httpLabel`
//! path segment — with the character-class form
//! (`arn:aws(?:-[a-z0-9]+)*:imagebuilder:[a-z0-9-]*[a-z0-9]:`). The same member name is
//! bound both ways by different operations of this model (`imagePipelineArn` is
//! `GetImagePipeline`'s query parameter and `StartImagePipelineExecution`'s body member), so
//! no rule over the violated member alone can place a refusal: the binding is a property of
//! the operation's own input shape, which is why these wordings are attached to the front
//! door each operation is served behind (`service_common::front_door::FrontDoor`) rather
//! than to the service-level refusal seam.
//!
//! Every other field of an expression — the account, the resource kind, the name, the
//! version — is quoted exactly as the model carries it, and a pattern that is not an ARN's
//! (`semanticVersion`, `stepExecutionId`) is quoted as the model writes it, anchors
//! included: there is nothing to restate in it.
//!
//! One operation states no expression at all ([`unstated_pattern_refusal`]): its own check
//! reports that the value does not match the pattern the parameter requires and echoes none
//! of it.

use service_common::validation::SubmittedEcho;
use service_common::validation_message::{
    api_parameter_path, must_be_at_most, must_be_one_of, must_match_pattern,
    parameter_value_mismatch, pattern_requirement_unstated,
};
use smithy_server_runtime::{Refused, Violation, ViolationKind};

/// The error code this service answers a parameter whose value it refuses with: the modelled
/// `InvalidParameterValueException` (HTTP 400), whose own documentation is "The value that
/// you provided for the specified parameter is invalid."
///
/// This model declares NO `ValidationException` — its only service-common error is
/// `ServiceException` (`smithy_query(query="service_info")`) — so the framework's synthetic
/// one is not an error this service can answer with.
pub const INVALID_PARAMETER_VALUE_EXCEPTION: &str = "InvalidParameterValueException";

/// The ARN partition field as the model spells it: `aws` followed by anything up to the
/// next colon, which is all a pattern needs in order to bound ONE field.
const MODELLED_PARTITION_FIELD: &str = "arn:aws[^:]*:";

/// The ARN Region field of this service as the model spells it: anything up to the next
/// colon, for the same reason as [`MODELLED_PARTITION_FIELD`].
const MODELLED_REGION_FIELD: &str = ":imagebuilder:[^:]+:";

/// The ARN partition field as a refusal of a BODY value states it: `aws` followed by the
/// hyphenated words a partition name is made of (`aws-cn`, `aws-us-gov`).
const REGION_CODE_PARTITION_FIELD: &str = "arn:aws(?:-[a-z]+)*:";

/// The ARN Region field as a refusal of a BODY value states it: the form a Region code takes
/// — an area, one or more hyphenated words, and a number (`us-east-1`, `ap-southeast-3`).
const REGION_CODE_REGION_FIELD: &str = ":imagebuilder:[a-z]{2,}(?:-[a-z]+)+-[0-9]+:";

/// The ARN partition field as a refusal of a value read out of the REQUEST LINE states it:
/// the characters a partition name is made of rather than the words.
const CHARACTER_CLASS_PARTITION_FIELD: &str = "arn:aws(?:-[a-z0-9]+)*:";

/// The ARN Region field as a refusal of a value read out of the REQUEST LINE states it: the
/// characters a Region code is made of, closing on one that is not a hyphen.
const CHARACTER_CLASS_REGION_FIELD: &str = ":imagebuilder:[a-z0-9-]*[a-z0-9]:";

/// How a refusal quotes the expression an ARN member's value had to match. See the module
/// docs: the form follows the layer that READ the value, not the member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatedPattern {
    /// The form a refusal of a value read out of the request BODY states.
    RegionCode,
    /// The form a refusal of a value read out of the REQUEST LINE states.
    CharacterClass,
    /// No expression at all: the requirement is stated as the pattern the parameter
    /// requires, with none of it echoed.
    Unstated,
}

impl StatedPattern {
    /// The requirement clause this form states for `member`, whose value had to match
    /// `pattern` as the model writes it.
    fn pattern_requirement(self, member: &str, pattern: &str) -> String {
        match self {
            Self::Unstated => pattern_requirement_unstated(member),
            Self::RegionCode => must_match_pattern(
                member,
                &pattern
                    .replace(MODELLED_PARTITION_FIELD, REGION_CODE_PARTITION_FIELD)
                    .replace(MODELLED_REGION_FIELD, REGION_CODE_REGION_FIELD),
            ),
            Self::CharacterClass => must_match_pattern(
                member,
                &pattern
                    .replace(MODELLED_PARTITION_FIELD, CHARACTER_CLASS_PARTITION_FIELD)
                    .replace(MODELLED_REGION_FIELD, CHARACTER_CLASS_REGION_FIELD),
            ),
        }
    }
}

/// The requirement clause this service states for a value below its member's `@range`
/// minimum: the bound the value had to reach.
///
/// Image Builder's own wording for this clause is "bigger than or equal to", which is not
/// how the shared statement of it reads ("greater than or equal to"), so it is stated here
/// rather than taken from `service_common::validation_message`. Its sibling — a value above
/// the maximum — is the shared clause ([`must_be_at_most`]), which Image Builder words the
/// same way the rest of the tree does.
fn must_be_at_least(member: &str, bound: &str) -> String {
    format!("{member} must be bigger than or equal to {bound}")
}

/// The requirement clause this service states for `violation`, naming the parameter as
/// `member` spells it, or `None` for a constraint class no recorded refusal of this service
/// establishes a clause for — which keeps the framework's own wording for it rather than
/// inventing one.
fn requirement(member: &str, violation: &Violation, form: StatedPattern) -> Option<String> {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => Some(form.pattern_requirement(member, pattern)),
        ViolationKind::MaxValue(bound) => Some(must_be_at_most(member, bound)),
        ViolationKind::MinValue(bound) => Some(must_be_at_least(member, bound)),
        ViolationKind::EnumValue(values) => Some(must_be_one_of(member, values)),
        _ => None,
    }
}

/// The refusal this service states for `refused`, quoting an ARN expression in `form`, or
/// `None` to leave the request to the layers behind the front door.
///
/// `None` unless EVERY violation is one this service states a requirement for: a request
/// that also broke a constraint of a class this service has not been seen to word is
/// refused by the framework's own walk, which states the constraint the value broke, rather
/// than being restated as one parameter's.
///
/// ONE parameter is named, the first the walk found: the sentence states the parameter whose
/// value was refused, and a sentence naming several would be a different sentence.
fn refusal(refused: &[(Violation, SubmittedEcho)], form: StatedPattern) -> Option<Refused> {
    let stated: Vec<(String, String)> = refused
        .iter()
        .map(|(violation, _)| {
            let member = api_parameter_path(&violation.path);
            requirement(&member, violation, form).map(|requirement| (member, requirement))
        })
        .collect::<Option<Vec<_>>>()?;
    let (member, requirement) = stated.first()?;
    Some(Refused {
        code: INVALID_PARAMETER_VALUE_EXCEPTION.to_owned(),
        message: parameter_value_mismatch(member, requirement),
    })
}

/// How this service refuses a value its request carried in the BODY.
pub fn body_parameter_refusal(refused: &[(Violation, SubmittedEcho)]) -> Option<Refused> {
    refusal(refused, StatedPattern::RegionCode)
}

/// How this service refuses a value its request carried in the REQUEST LINE — an
/// `@httpQuery` parameter, an `@httpLabel` path segment.
pub fn uri_parameter_refusal(refused: &[(Violation, SubmittedEcho)]) -> Option<Refused> {
    refusal(refused, StatedPattern::CharacterClass)
}

/// How the one operation whose own check echoes no expression refuses a value its request
/// carried in the REQUEST LINE: the parameter, and the fact that it does not match the
/// pattern that parameter requires.
pub fn unstated_pattern_refusal(refused: &[(Violation, SubmittedEcho)]) -> Option<Refused> {
    refusal(refused, StatedPattern::Unstated)
}

#[cfg(test)]
mod tests {
    //! The sentence over the constraint classes and the pattern forms it is stated for. The
    //! wire behaviour — that every operation's refusal carries it, with the parameter the
    //! request named — is asserted through the router in
    //! `tests/all/parameter_value_refusals.rs`.

    use super::*;

    /// One violation of `kind` at `path`, for the value a request sent.
    fn violation(path: &str, kind: ViolationKind) -> (Violation, SubmittedEcho) {
        (
            Violation {
                path: path.to_owned(),
                kind,
                value: Some("submitted".to_owned()),
                sensitive: false,
            },
            SubmittedEcho::Rendered("submitted".to_owned()),
        )
    }

    /// The modelled `ImagePipelineArn` pattern
    /// (`smithy_query(query="shape",shape="ImagePipelineArn")`), the one member this model
    /// binds both to a request line and to a body.
    const MODELLED_IMAGE_PIPELINE_ARN: &str =
        "^arn:aws[^:]*:imagebuilder:[^:]+:(?:[0-9]{12}|aws):image-pipeline/[a-z0-9-_]+$";

    /// RULE: the same refused value states the expression in the form the layer that read it
    /// writes, and every other field of the expression is the model's.
    #[test]
    fn an_arn_expression_is_quoted_in_the_form_of_the_layer_that_read_it() {
        // (case, the form, the expression the refusal quotes)
        let rows = [
            (
                "a body member",
                StatedPattern::RegionCode,
                "^arn:aws(?:-[a-z]+)*:imagebuilder:[a-z]{2,}(?:-[a-z]+)+-[0-9]+:\
                 (?:[0-9]{12}|aws):image-pipeline/[a-z0-9-_]+$",
            ),
            (
                "a request-line parameter",
                StatedPattern::CharacterClass,
                "^arn:aws(?:-[a-z0-9]+)*:imagebuilder:[a-z0-9-]*[a-z0-9]:\
                 (?:[0-9]{12}|aws):image-pipeline/[a-z0-9-_]+$",
            ),
        ];

        for (case, form, expression) in rows {
            let refused = [violation(
                "imagePipelineArn",
                ViolationKind::Pattern(MODELLED_IMAGE_PIPELINE_ARN.to_owned()),
            )];
            let stated = refusal(&refused, form).expect("a stated refusal");
            let expected = format!(
                "The value supplied for parameter 'imagePipelineArn' is not valid. \
                 imagePipelineArn must match pattern {expression}"
            );
            assert_eq!(
                stated.code, INVALID_PARAMETER_VALUE_EXCEPTION,
                "case {case:?}: expected this service's own code, got {:?}",
                stated.code
            );
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }

        // The operation that echoes no expression states the requirement without one,
        // whatever the pattern.
        let refused = [violation(
            "distributionConfigurationArn",
            ViolationKind::Pattern(MODELLED_IMAGE_PIPELINE_ARN.to_owned()),
        )];
        let stated = refusal(&refused, StatedPattern::Unstated).expect("a stated refusal");
        assert_eq!(
            stated.message,
            "The value supplied for parameter 'distributionConfigurationArn' is not valid. \
             distributionConfigurationArn does not match the required pattern",
            "case \"an unstated expression\": got {:?}",
            stated.message
        );
    }

    /// RULE: a pattern that is not an ARN's is quoted as the model writes it — there is no
    /// partition or Region field in it to restate — in every form.
    #[test]
    fn a_pattern_that_is_not_an_arns_is_quoted_as_the_model_writes_it() {
        for form in [StatedPattern::RegionCode, StatedPattern::CharacterClass] {
            let refused = [violation(
                "semanticVersion",
                ViolationKind::Pattern(r"^[0-9]+\.[0-9]+\.[0-9]+$".to_owned()),
            )];
            let stated = refusal(&refused, form).expect("a stated refusal");
            assert_eq!(
                stated.message,
                "The value supplied for parameter 'semanticVersion' is not valid. \
                 semanticVersion must match pattern ^[0-9]+\\.[0-9]+\\.[0-9]+$",
                "case {form:?}: got {:?}",
                stated.message
            );
        }
    }

    /// RULE: every constraint class this service states a requirement for is stated in the
    /// one sentence, naming the parameter as the API addresses it; a class it states none
    /// for leaves the whole refusal to the framework.
    #[test]
    fn each_constraint_class_states_its_own_requirement() {
        // (case, the violation, the sentence — or `None` to leave it to the framework)
        let rows: Vec<(&str, (Violation, SubmittedEcho), Option<&str>)> = vec![
            (
                "an upper bound inside a structure member",
                violation(
                    "imageTestsConfiguration.timeoutMinutes",
                    ViolationKind::MaxValue("1440".to_owned()),
                ),
                Some(
                    "The value supplied for parameter 'imageTestsConfiguration.timeoutMinutes' \
                     is not valid. imageTestsConfiguration.timeoutMinutes must be smaller than \
                     or equal to 1440",
                ),
            ),
            (
                "a lower bound",
                violation("maxResults", ViolationKind::MinValue("1".to_owned())),
                Some(
                    "The value supplied for parameter 'maxResults' is not valid. maxResults \
                     must be bigger than or equal to 1",
                ),
            ),
            (
                "an enumerated value set",
                violation(
                    "owner",
                    ViolationKind::EnumValue(vec![
                        "Self".to_owned(),
                        "Shared".to_owned(),
                        "Amazon".to_owned(),
                        "ThirdParty".to_owned(),
                        "AWSMarketplace".to_owned(),
                    ]),
                ),
                Some(
                    "The value supplied for parameter 'owner' is not valid. owner must be one \
                     of ['Self', 'Shared', 'Amazon', 'ThirdParty', 'AWSMarketplace']",
                ),
            ),
            (
                "a pattern broken inside a list element",
                violation(
                    "distributions.1.member.amiDistributionConfiguration.name",
                    ViolationKind::Pattern("^[-_A-Za-z0-9]+$".to_owned()),
                ),
                Some(
                    "The value supplied for parameter \
                     'distributions[0].amiDistributionConfiguration.name' is not valid. \
                     distributions[0].amiDistributionConfiguration.name must match pattern \
                     ^[-_A-Za-z0-9]+$",
                ),
            ),
            (
                "a length bound, which this service has not been seen to state",
                violation("description", ViolationKind::MaxLength(1024)),
                None,
            ),
            (
                "a member the shape requires and the request omits",
                violation("clientToken", ViolationKind::MissingRequiredMember),
                None,
            ),
        ];

        for (case, refused, expected) in &rows {
            let stated = refusal(std::slice::from_ref(refused), StatedPattern::RegionCode);
            match expected {
                Some(message) => {
                    let stated =
                        stated.unwrap_or_else(|| panic!("case {case:?}: expected a refusal"));
                    assert_eq!(
                        stated.message, *message,
                        "case {case:?}: expected {message:?}, got {:?}",
                        stated.message
                    );
                }
                None => assert!(
                    stated.is_none(),
                    "case {case:?}: expected the framework's own wording, got {stated:?}"
                ),
            }
        }

        // A request that breaks one constraint this service states and one it does not is
        // left to the framework as a whole: the sentence names ONE parameter, and the other
        // violation would go unstated.
        let mixed = [
            violation(
                "semanticVersion",
                ViolationKind::Pattern("^[0-9]+$".to_owned()),
            ),
            violation("description", ViolationKind::MaxLength(1024)),
        ];
        assert!(
            refusal(&mixed, StatedPattern::RegionCode).is_none(),
            "case \"a stated class beside an unstated one\": expected the framework's own \
             wording"
        );
    }
}
