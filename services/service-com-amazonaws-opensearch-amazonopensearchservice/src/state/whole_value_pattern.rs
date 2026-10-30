// SPDX-License-Identifier: CC0-1.0

//! Whether a value satisfies its member's modelled `@pattern` over its WHOLE length.
//!
//! Smithy defines the trait as a FIND — the expression has to match SOMEWHERE in the
//! value — which coincides with the whole-value reading for every expression a model
//! anchors with `^` / `$` and is weaker for an unanchored one. AWS applies the
//! expression to the whole value, so an unanchored pattern refuses values a find
//! accepts, and the request boundary of this emulator (which runs the trait as Smithy
//! defines it) lets them through.
//!
//! This model anchors every string pattern a direct query data source carries except
//! `PolicyDocument`'s `.*`
//! (`smithy_query(query="operation",operation="AddDirectQueryDataSource")`): a dot
//! matches no line terminator, so a policy document written across several lines is
//! outside that expression as a whole while the expression is found in it. That member
//! — `DataSourceAccessPolicy`, which the add and the update both carry — is therefore
//! the one this service checks itself.
//!
//! NOTHING about constraint checking is reimplemented here: the member's own modelled
//! expression is read off its generated schema, anchored, re-declared in the
//! framework's own form (a [`TraitMap`] carrying a [`PatternTrait`]) on a member schema
//! of the same name, and the check is [`validate`] — the same walk the request boundary
//! runs — reported through the framework's own envelope. The clause quotes the MODEL's
//! expression rather than the anchored form the check used, because the anchors express
//! how AWS applies the pattern and are no part of what the model declares.

use std::sync::LazyLock;

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use ::aws_smithy_schema_server::constraints::{validate, PatternTrait};
use ::aws_smithy_schema_server::{envelope, Violation, ViolationKind};

/// This service's Smithy namespace, for the shadow member schema below.
const NAMESPACE: &str = "com.amazonaws.opensearch";

/// The member the policy document is carried at, as the model names it — the name the
/// refusal's clause states (lowerCamel, the casing the validation path uses).
const MEMBER: &str = "DataSourceAccessPolicy";

/// The largest document one clause quotes in full: `PolicyDocument`'s own
/// `@length(min: 0, max: 102400)`
/// (`smithy_query(query="shape",shape="PolicyDocument")`), so the ceiling is a MODEL
/// constant rather than a request-sized one — an AWS constraint clause quotes the value
/// it refused, and this member's declared maximum is far above the shared default.
const MAX_QUOTED: usize = 102_400;

/// The `@pattern` the model declares for the policy document member, or `None` when it
/// declares none — which is nothing to check.
///
/// Read off the member's own generated schema, so it cannot drift from the model.
static MODELLED_PATTERN: LazyLock<Option<&'static str>> = LazyLock::new(|| {
    modelled_member_pattern(
        crate::ops::types::ADD_DIRECT_QUERY_DATA_SOURCE_INPUT_SCHEMA,
        MEMBER,
    )
});

/// [`MODELLED_PATTERN`] anchored at both ends, inside a non-capturing group that keeps a
/// top-level alternation within the anchors, so the WHOLE value has to match it.
static ANCHORED_PATTERN: LazyLock<Option<String>> =
    LazyLock::new(|| MODELLED_PATTERN.map(|modelled| format!("^(?:{modelled})$")));

/// The shadow member's constraints: the anchored pattern and nothing else — the
/// member's `@length` is enforced by the boundary walk already, and checking it twice
/// would state it twice.
static MEMBER_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    if let Some(anchored) = ANCHORED_PATTERN.as_deref() {
        traits.insert(Box::new(PatternTrait::new(anchored)));
    }
    traits
});

/// The member schema the walk reads: its name gives the violation's path and its trait
/// map carries the constraint.
static M_POLICY_DOCUMENT: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.opensearch#WholeValuePolicyDocument$DataSourceAccessPolicy",
        NAMESPACE,
        MEMBER,
    ),
    ShapeType::String,
    MEMBER,
    0,
)
.with_traits(&MEMBER_TRAITS);

/// The root of the shadow input the walk is driven over. Its member list is empty on
/// purpose: the walk visits what [`WholeValuePolicyDocument`] WRITES, and a root struct
/// schema carries no member name, so it contributes no path segment.
static WHOLE_VALUE_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "com.amazonaws.opensearch#WholeValuePolicyDocument",
        NAMESPACE,
        "WholeValuePolicyDocument",
    ),
    ShapeType::Structure,
    &[],
);

/// One request's policy document, written for the validating walk the way the codec
/// writes a string member.
struct WholeValuePolicyDocument<'a> {
    document: &'a str,
}

impl SerializableStruct for WholeValuePolicyDocument<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&M_POLICY_DOCUMENT, self.document)
    }
}

/// The constraint report for a policy document whose WHOLE value is outside its
/// member's modelled `@pattern`, and `None` for one that satisfies it.
///
/// The message is the framework's own envelope over the framework's own clause, with
/// the value quoted as the caller submitted it (up to [`MAX_QUOTED`]) rather than as the
/// walk's bounded echo of it, so the refusal reads like every other constraint refusal
/// of this service.
pub fn policy_document_refusal(document: &str) -> Option<String> {
    let modelled = (*MODELLED_PATTERN)?;
    let input = WholeValuePolicyDocument { document };
    // The shadow member carries ONE constraint, so every violation the walk can report
    // is about that pattern; each is restated with the expression the MODEL declares.
    let violations: Vec<Violation> = validate(&input, &WHOLE_VALUE_SCHEMA)
        .into_iter()
        .map(|violation| Violation {
            kind: ViolationKind::Pattern(modelled.to_string()),
            ..violation
        })
        .collect();
    let violations = ::service_common::validation::with_submitted_values_up_to(
        violations,
        &input,
        &WHOLE_VALUE_SCHEMA,
        MAX_QUOTED,
    );
    envelope(&violations)
}

/// The `@pattern` expression the model declares for one member of `input_schema`, or
/// `None` for a member it declares none for (or does not declare at all).
fn modelled_member_pattern(input_schema: &Schema, member: &str) -> Option<&'static str> {
    let member_schema = input_schema
        .members()
        .iter()
        .find(|candidate| candidate.member_name() == Some(member))?;
    let pattern = member_schema.traits()?.get(&PatternTrait::TRAIT_ID)?;
    Some(pattern.as_any().downcast_ref::<PatternTrait>()?.regex)
}
