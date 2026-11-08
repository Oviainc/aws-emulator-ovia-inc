// SPDX-License-Identifier: CC0-1.0

//! WHAT a scalar in a request body states: the value each of JSON's scalar types
//! states for a member of each modelled scalar type, and the declared spelling of
//! a member this service reads without regard to case.
//!
//! RULE: Amazon EKS reads a body leniently. A member whose modelled type is a
//! number or a boolean is read from the string that spells that value, a member
//! whose modelled type is a string is read from the number or boolean the body
//! carries there, and the members this service reads without regard to case —
//! `kubernetesNetworkConfig.ipFamily` and the add-on operations'
//! `resolveConflicts` — name one of the values their own enum declares whatever
//! case it is spelled in. A value that states nothing of the modelled type is
//! still refused.
//!
//! These are the value rules alone — no request, no codec and no state — so they
//! are stated and tested here once; [`super::lenient_json`] is where they are
//! applied, at the one seam that runs before the framework judges a body.

use aws_smithy_schema::serde::SerdeError;
use aws_smithy_schema::Schema;
use aws_smithy_types::{Document, Number};

use crate::ops::types::{IP_FAMILY_VALUES, RESOLVE_CONFLICTS_VALUES};

/// The members this service reads without regard to case, as `(the modelled
/// member name, the values it is read against)`.
///
/// The value set is the generated constant of the member's own enum shape, so this
/// rule and the framework's validating walk cannot disagree about what the model
/// declares. The table is deliberately a list of MEMBERS rather than a rule about
/// enums: every other enum-backed member of this service is judged by its value
/// set exactly as declared — see [`super::off_enum_member_contract`], which states
/// that rule and the refusal it earns.
///
/// `ipFamily` is in it because that is how this service reads it: `CreateCluster`
/// compares the family a request asks for to the modelled member ignoring case,
/// and the recorded service serves a create that spells it `IPv4`.
///
/// `resolveConflicts` is in it for the same reason: the recorded service serves a
/// `CreateAddon` whose conflict-resolution option is spelled `overwrite`, so the
/// value names the declared member it spells rather than naming nothing. Both
/// operations that take the member (`CreateAddon`, `UpdateAddon`) target the same
/// enum, so one entry reads it the same way for both.
///
/// It is keyed by the modelled member NAME because that is what a member schema
/// carries: the generated schema of a structure member holds the member's own
/// shape id and kind, not the id of the shape it targets
/// (`vendor/smithy-macros/src/lib.rs` emits `Schema::new_member(...)` with the
/// member's own id), so the enum behind it is not reachable from here. Each name
/// identifies one member of this model's input shapes today
/// (`KubernetesNetworkConfigRequest$ipFamily`, and
/// `CreateAddonRequest$resolveConflicts`/`UpdateAddonRequest$resolveConflicts`,
/// which target one enum between them; the only other member named `ipFamily`
/// belongs to the response shape, which is never read from a request), and the
/// rule fires only for a value that IS one of the declared values in some other
/// case — so a member of that name on another shape could at worst have a value of
/// this value set re-spelled. The reconciliation is for the generated member
/// schema to carry its target's shape id, which a service crate cannot add.
const CASE_INSENSITIVE_MEMBERS: &[(&str, &[&str])] = &[
    ("ipFamily", IP_FAMILY_VALUES),
    ("resolveConflicts", RESOLVE_CONFLICTS_VALUES),
];

/// The declared spelling of `value` for `member`, when that member is read without
/// regard to case and the value is one of its declared values in some other case.
///
/// `None` leaves the value exactly as the client sent it, for the walk to accept or
/// refuse as it would have — which is what every member outside
/// [`CASE_INSENSITIVE_MEMBERS`], and every value that is no declared one in any
/// case, gets.
pub fn declared_spelling(member: &Schema, value: &str) -> Option<&'static str> {
    let name = member.member_name()?;
    let (_, declared) = CASE_INSENSITIVE_MEMBERS
        .iter()
        .find(|(cased, _)| *cased == name)?;
    if declared.contains(&value) {
        return None;
    }
    declared
        .iter()
        .find(|spelling| spelling.eq_ignore_ascii_case(value))
        .copied()
}

/// Whether a member's value comes from the request itself — a URI label, the query
/// string, a header or the raw payload — rather than from the JSON body.
///
/// Only the body states a value in one of JSON's scalar types, so only a body
/// member can state one in another: a request-bound value is TEXT either way, and
/// is read by the binding deserializer, which is the whole rule for it. The
/// members of a nested shape are never bound, so this is `false` for every one of
/// them.
pub fn reads_from_the_request(member: &Schema) -> bool {
    member.http_label().is_some()
        || member.http_query().is_some()
        || member.http_query_params().is_some()
        || member.http_header().is_some()
        || member.http_prefix_headers().is_some()
        || member.http_payload().is_some()
        || member.http_response_code().is_some()
}

/// The class of value a document holds, as a refusal names it.
fn class_of(document: &Document) -> &'static str {
    match document {
        Document::Null => "null",
        Document::Bool(_) => "a boolean",
        Document::Number(_) => "a number",
        Document::String(_) => "a string",
        Document::Array(_) => "an array",
        Document::Object(_) => "an object",
    }
}

/// The refusal a value that states nothing of the modelled type earns.
///
/// It names the CLASS of value the body carried and the type the member reads —
/// the two facts a caller has to reconcile — and never the value itself, so no
/// value a caller chose the length of is echoed back.
pub fn not_stated(document: &Document, target: &str) -> SerdeError {
    SerdeError::custom(format!(
        "expected {target}, got {} that states none",
        class_of(document)
    ))
}

/// The boolean a document states: the boolean it is, or the one its text spells.
pub fn boolean_of(document: &Document) -> Option<bool> {
    match document {
        Document::Bool(held) => Some(*held),
        Document::String(text) => match text.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// The whole number a document states: the number it is, or the one its text
/// spells. A fraction or a value past the range states no whole number, exactly as
/// the codec's own reading of one does.
pub fn integral_of(document: &Document) -> Option<i64> {
    match document {
        Document::Number(number) => match number {
            Number::PosInt(held) => i64::try_from(*held).ok(),
            Number::NegInt(held) => Some(*held),
            Number::Float(held) => (held.fract() == 0.0
                && (i64::MIN as f64..=i64::MAX as f64).contains(held))
            .then_some(*held as i64),
        },
        Document::String(text) => text.parse().ok(),
        _ => None,
    }
}

/// The real number a document states: the number it is, or the one its text
/// spells — including the non-finite values JSON can only carry as text.
pub fn double_of(document: &Document) -> Option<f64> {
    match document {
        Document::Number(number) => Some(match number {
            Number::PosInt(held) => *held as f64,
            Number::NegInt(held) => *held as f64,
            Number::Float(held) => *held,
        }),
        Document::String(text) => text.parse().ok(),
        _ => None,
    }
}

/// The text a document states: the string it is, or the text of the number or
/// boolean it carries, which is the text that value was written with.
pub fn text_of(document: &Document) -> Option<String> {
    match document {
        Document::String(text) => Some(text.clone()),
        Document::Bool(held) => Some(held.to_string()),
        Document::Number(number) => Some(match number {
            Number::PosInt(held) => held.to_string(),
            Number::NegInt(held) => held.to_string(),
            Number::Float(held) => held.to_string(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{boolean_of, declared_spelling, double_of, integral_of, text_of};
    use crate::ops::types::{
        KubernetesNetworkConfigRequest, VpcConfigRequest, CREATE_ADDON_INPUT_SCHEMA,
        UPDATE_ADDON_INPUT_SCHEMA,
    };
    use aws_smithy_schema::Schema;
    use aws_smithy_types::{Document, Number};

    /// Every scalar document, plus the two kinds that are no scalar at all, so each
    /// rule below is stated over the same set.
    fn documents() -> Vec<(&'static str, Document)> {
        vec![
            ("a boolean", Document::Bool(true)),
            ("the other boolean", Document::Bool(false)),
            (
                "a positive whole number",
                Document::Number(Number::PosInt(30_000)),
            ),
            (
                "a negative whole number",
                Document::Number(Number::NegInt(-7)),
            ),
            ("a fraction", Document::Number(Number::Float(1.5))),
            ("a whole float", Document::Number(Number::Float(2.0))),
            (
                "a string spelling a boolean",
                Document::String("true".to_string()),
            ),
            (
                "a string spelling a whole number",
                Document::String("30100".to_string()),
            ),
            (
                "a string spelling a fraction",
                Document::String("1.33".to_string()),
            ),
            ("a word", Document::String("many".to_string())),
            ("a padded number", Document::String(" 30100".to_string())),
            ("the empty string", Document::String(String::new())),
            ("null", Document::Null),
            ("an array", Document::Array(Vec::new())),
            (
                "an object",
                Document::Object(std::collections::HashMap::new()),
            ),
        ]
    }

    /// RULE: a boolean member reads the boolean it is or the boolean its text
    /// spells, and nothing else.
    #[test]
    fn a_boolean_is_read_from_a_boolean_or_from_the_text_that_spells_one() {
        let expected = |case: &str| match case {
            "a boolean" | "a string spelling a boolean" => Some(true),
            "the other boolean" => Some(false),
            _ => None,
        };
        for (case, document) in documents() {
            let actual = boolean_of(&document);
            assert_eq!(
                actual,
                expected(case),
                "case {case:?}: expected {:?}, got {actual:?}",
                expected(case)
            );
        }
    }

    /// RULE: a whole-number member reads the whole number it is or the one its text
    /// spells; a fraction states none, as the codec's own reading of one does.
    #[test]
    fn a_whole_number_is_read_from_a_number_or_from_the_text_that_spells_one() {
        let expected = |case: &str| match case {
            "a positive whole number" => Some(30_000),
            "a negative whole number" => Some(-7),
            "a whole float" => Some(2),
            "a string spelling a whole number" => Some(30_100),
            _ => None,
        };
        for (case, document) in documents() {
            let actual = integral_of(&document);
            assert_eq!(
                actual,
                expected(case),
                "case {case:?}: expected {:?}, got {actual:?}",
                expected(case)
            );
        }
    }

    /// RULE: a real-number member reads any number, and the one its text spells.
    #[test]
    fn a_real_number_is_read_from_a_number_or_from_the_text_that_spells_one() {
        let expected = |case: &str| match case {
            "a positive whole number" => Some(30_000.0),
            "a negative whole number" => Some(-7.0),
            "a fraction" => Some(1.5),
            "a whole float" => Some(2.0),
            "a string spelling a whole number" => Some(30_100.0),
            "a string spelling a fraction" => Some(1.33),
            _ => None,
        };
        for (case, document) in documents() {
            let actual = double_of(&document);
            assert_eq!(
                actual,
                expected(case),
                "case {case:?}: expected {:?}, got {actual:?}",
                expected(case)
            );
        }
    }

    /// RULE: a string member reads the text of whichever scalar is there — and a
    /// structure or a list states no text at all.
    #[test]
    fn a_string_is_read_from_the_text_of_any_scalar() {
        let expected = |case: &str| match case {
            "a boolean" | "a string spelling a boolean" => Some("true"),
            "the other boolean" => Some("false"),
            "a positive whole number" => Some("30000"),
            "a negative whole number" => Some("-7"),
            "a fraction" => Some("1.5"),
            "a whole float" => Some("2"),
            "a string spelling a whole number" => Some("30100"),
            "a string spelling a fraction" => Some("1.33"),
            "a word" => Some("many"),
            "a padded number" => Some(" 30100"),
            "the empty string" => Some(""),
            _ => None,
        };
        for (case, document) in documents() {
            let actual = text_of(&document);
            assert_eq!(
                actual.as_deref(),
                expected(case),
                "case {case:?}: expected {:?}, got {actual:?}",
                expected(case)
            );
        }
    }

    /// The member schema of `name` on `schema`, read off the generated input shape
    /// so a test names the member the same way the codec does.
    fn member_of(schema: &'static Schema, name: &str) -> &'static Schema {
        schema
            .members()
            .iter()
            .copied()
            .find(|member| member.member_name() == Some(name))
            .unwrap_or_else(|| panic!("the shape must declare a {name} member"))
    }

    /// RULE: the member this service reads without regard to case is restated in
    /// the spelling the model declares; every other member, and every value that is
    /// no declared one in any case, is left exactly as sent.
    #[test]
    fn only_the_case_insensitive_member_is_restated_and_only_onto_a_declared_value() {
        let ip_family = member_of(&KubernetesNetworkConfigRequest::SCHEMA, "ipFamily");
        // Rows: both members of `IpFamily` (`smithy_query(query="shape",
        // shape="IpFamily")`: `ipv4 | ipv6`) in several cases, then the values that
        // name no family at all.
        let rows: [(&str, Option<&str>); 11] = [
            ("ipv4", None),
            ("IPv4", Some("ipv4")),
            ("IPV4", Some("ipv4")),
            ("Ipv4", Some("ipv4")),
            ("ipv6", None),
            ("IPv6", Some("ipv6")),
            ("IPV6", Some("ipv6")),
            ("ipv5", None),
            (" ipv4", None),
            ("ip-v4", None),
            ("", None),
        ];
        for (value, expected) in rows {
            let actual = declared_spelling(ip_family, value);
            assert_eq!(
                actual, expected,
                "case {value:?}: expected {expected:?}, got {actual:?}"
            );
        }

        // A member of another name is never restated, whatever it spells — this one
        // takes a CIDR list and an egress mode, neither of which this service reads
        // without regard to case.
        let egress = member_of(&VpcConfigRequest::SCHEMA, "controlPlaneEgressMode");
        for value in ["AWS_MANAGED", "aws_managed", "Customer_Routed"] {
            let actual = declared_spelling(egress, value);
            assert_eq!(
                actual, None,
                "case {value:?}: a member outside the table keeps its value, got {actual:?}"
            );
        }
    }

    /// The same rule on the other member of the table, read off the two input
    /// shapes that carry it: a value that spells a declared conflict-resolution
    /// option in another case is restated in the spelling the model declares, and
    /// a value that spells no declared one is left exactly as sent.
    #[test]
    fn the_conflict_resolution_member_is_restated_onto_the_declared_option() {
        // Rows: every value of `ResolveConflicts` (source: the generated
        // `RESOLVE_CONFLICTS_VALUES`, `smithy_query query="shape"
        // shape="ResolveConflicts"`) in several cases, then values that spell no
        // declared option at all.
        let rows: [(&str, Option<&str>); 11] = [
            ("OVERWRITE", None),
            ("overwrite", Some("OVERWRITE")),
            ("Overwrite", Some("OVERWRITE")),
            ("NONE", None),
            ("none", Some("NONE")),
            ("None", Some("NONE")),
            ("PRESERVE", None),
            ("Preserve", Some("PRESERVE")),
            ("BANANA", None),
            (" NONE", None),
            ("", None),
        ];
        for schema in [CREATE_ADDON_INPUT_SCHEMA, UPDATE_ADDON_INPUT_SCHEMA] {
            let member = member_of(schema, "resolveConflicts");
            for (value, expected) in rows {
                let actual = declared_spelling(member, value);
                assert_eq!(
                    actual, expected,
                    "case {value:?}: expected {expected:?}, got {actual:?}"
                );
            }
        }
    }
}
