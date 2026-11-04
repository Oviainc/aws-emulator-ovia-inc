// SPDX-License-Identifier: CC0-1.0

//! The template parameters whose VALUE lives in AWS Systems Manager Parameter Store, and
//! the resolution a deployment performs on them.
//!
//! ## The rule this module implements
//!
//! A template parameter declared with a type of the form
//! `AWS::SSM::Parameter::Value<<type>>` does not carry its own value: the value a request
//! supplies for it (or the `Default` the template declares for it) is the NAME of a
//! Parameter Store parameter, and CloudFormation reads that parameter when the stack is
//! deployed and substitutes the value it holds — "the parameter value is the name of a
//! Systems Manager parameter, and the value that CloudFormation uses is the value of that
//! parameter". So a `Ref` to such a parameter, an `Fn::Sub` naming it, and every resource
//! property built from it all carry the VALUE the store holds, never the name that
//! addressed it; the name is what the stack reports as the parameter's `ParameterValue`
//! and the value is what it reports as its `ResolvedValue`.
//!
//! The inner type is what the resolved value is then read AS: a `List<…>` or a
//! `CommaDelimitedList` inside the angle brackets makes the resolved value a list to every
//! function that reads it, exactly as a parameter declared with that type directly would be
//! ([`parameter_types_in_force`] is what every evaluation scope takes its types from).
//!
//! A name the store does not answer for leaves the name itself the value in force: one
//! unanswered read is no evidence about the parameter, and nothing here invents a value
//! for it.
//!
//! The store is read through the SAME in-process, signed call a resource's own create and a
//! `{{resolve:ssm:…}}` dynamic reference ride on ([`super::provisioner::call_ssm`]), so a
//! parameter resolves against the very parameters `PutParameter` wrote rather than against a
//! second copy of them.
//!
//! ## Bounds
//!
//! One read per DISTINCT name the deployment's parameters address, and a template declares
//! at most [`super::template::MAX_PARAMETERS_PER_TEMPLATE`] parameters (the documented
//! per-template quota, enforced where the body is read), so the reads one request costs are
//! bounded by that quota however many times a name is repeated.

use std::collections::BTreeMap;

use serde_json::json;

use super::provisioner::call_ssm;
use super::StoredParameter;

/// The opening of the declared `Type` of a parameter whose value Parameter Store holds:
/// `AWS::SSM::Parameter::Value<` … `>`, as the CloudFormation User Guide spells the SSM
/// parameter types.
const SSM_VALUE_TYPE_OPEN: &str = super::dynamic_references::SSM_PARAMETER_TYPE;

/// See [`SSM_VALUE_TYPE_OPEN`].
const SSM_VALUE_TYPE_CLOSE: &str = ">";

/// The Systems Manager operation one parameter is read with.
const GET_PARAMETER: &str = "GetParameter";

/// The type a parameter declared `AWS::SSM::Parameter::Value<<type>>` resolves to a value
/// OF: the type inside the angle brackets.
///
/// `None` for a declared type that is not an SSM-typed one at all — such a parameter
/// carries its own value, and nothing is read for it.
pub fn resolved_as(declared_type: &str) -> Option<&str> {
    declared_type
        .strip_prefix(SSM_VALUE_TYPE_OPEN)?
        .strip_suffix(SSM_VALUE_TYPE_CLOSE)
}

/// The type each of a template's parameters is read AS by the expressions that resolve it:
/// the type the template declares, except for an SSM-typed parameter, which is read as the
/// type INSIDE its angle brackets.
///
/// `AWS::SSM::Parameter::Value<List<String>>` resolves to a list for the same reason
/// `List<String>` does: what the parameter carries, once [`resolve`] has run, is the value
/// the store holds rather than the name that addressed it, so the inner type is the type of
/// the value in force. The companion of [`super::parameter_value_map`] — the value in force
/// and the type it is read as are the two halves of one resolution, so every evaluation
/// scope is built from both.
pub fn parameter_types_in_force(
    declared: &super::template::Template,
) -> BTreeMap<String, String> {
    declared
        .parameter_types()
        .into_iter()
        .map(|(key, declared_type)| {
            let in_force = resolved_as(&declared_type)
                .map_or_else(|| declared_type.clone(), str::to_string);
            (key, in_force)
        })
        .collect()
}

/// Resolve every SSM-typed parameter of a deployment against Parameter Store, recording the
/// value the store holds as that parameter's [`StoredParameter::resolved_value`].
///
/// A name the store does not answer for leaves that parameter with no resolved value, so the
/// name it addressed stays the value in force: one unanswered read is no evidence about the
/// parameter, and nothing here invents a value for it.
///
/// Idempotent in the store: this only READS Parameter Store, and the deployment's own
/// records are what carry the result.
pub async fn resolve(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    template_body: &str,
    parameters: &mut [StoredParameter],
) {
    let declared = super::template::parse(template_body).unwrap_or_default();
    let declared_types = declared.parameter_types();
    // The names this deployment addresses, in the order its parameters address them and
    // each read ONCE however many parameters address it.
    let mut addressed: Vec<&str> = Vec::new();
    for parameter in parameters
        .iter()
        .filter(|parameter| is_ssm_typed(&declared_types, &parameter.key))
    {
        if !addressed.contains(&parameter.value.as_str()) {
            addressed.push(parameter.value.as_str());
        }
    }
    if addressed.is_empty() {
        return;
    }
    let mut held: BTreeMap<String, String> = BTreeMap::new();
    for name in addressed {
        if let Some(value) = value_of(ctx, region, name).await {
            held.insert(name.to_string(), value);
        }
    }
    for parameter in parameters {
        if !is_ssm_typed(&declared_types, &parameter.key) {
            continue;
        }
        parameter.resolved_value = held.get(&parameter.value).cloned();
    }
}

/// Whether the template declares `key` with an SSM-typed `Type`, so the value in force for
/// it is the NAME of a Parameter Store parameter.
fn is_ssm_typed(declared_types: &BTreeMap<String, String>, key: &str) -> bool {
    declared_types
        .get(key)
        .is_some_and(|declared| resolved_as(declared).is_some())
}

/// The value Parameter Store holds under `name`, or `None` where it holds none.
///
/// The read is `GetParameter` on that service's own wire, so a name it does not hold
/// answers with its own `ParameterNotFound` and nothing here guesses at a value. The name is
/// passed verbatim: Parameter Store itself reads a `name:version` suffix as a version
/// selector and a full ARN as the parameter it names, so neither spelling is re-parsed here.
async fn value_of(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let answer = call_ssm(ctx, region, GET_PARAMETER, &json!({ "Name": name }))
        .await
        .ok()?;
    answer
        .pointer("/Parameter/Value")
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SSM parameter types the CloudFormation User Guide publishes, beside the types a
    /// parameter carries its own value with: what each resolves a value OF, and which of
    /// them is a Parameter Store lookup at all.
    #[test]
    fn an_ssm_typed_parameter_resolves_a_value_of_its_inner_type() {
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "a single string",
                "AWS::SSM::Parameter::Value<String>",
                Some("String"),
            ),
            (
                "a list of strings",
                "AWS::SSM::Parameter::Value<List<String>>",
                Some("List<String>"),
            ),
            (
                "a comma-delimited list",
                "AWS::SSM::Parameter::Value<CommaDelimitedList>",
                Some("CommaDelimitedList"),
            ),
            (
                "an AWS-specific type",
                "AWS::SSM::Parameter::Value<AWS::EC2::Image::Id>",
                Some("AWS::EC2::Image::Id"),
            ),
            (
                "a list of an AWS-specific type",
                "AWS::SSM::Parameter::Value<List<AWS::EC2::Subnet::Id>>",
                Some("List<AWS::EC2::Subnet::Id>"),
            ),
            // The parameter type that names a parameter KEY rather than resolving its
            // value: "AWS::SSM::Parameter::Name — the name of a Systems Manager parameter
            // key", which a `Ref` answers with as written.
            ("a parameter name", "AWS::SSM::Parameter::Name", None),
            ("a plain string", "String", None),
            ("a plain list", "CommaDelimitedList", None),
            (
                "an AWS-specific type of its own",
                "AWS::EC2::Image::Id",
                None,
            ),
        ];
        for (case, declared, expected) in rows {
            let resolved = resolved_as(declared);
            assert_eq!(
                resolved, *expected,
                "case {case:?}: expected {expected:?}, got {resolved:?}"
            );
        }
    }
}
