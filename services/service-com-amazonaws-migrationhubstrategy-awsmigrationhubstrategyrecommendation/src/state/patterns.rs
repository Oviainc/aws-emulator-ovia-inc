// SPDX-License-Identifier: CC0-1.0

//! A member's modelled `@pattern` holds over the WHOLE value.
//!
//! Smithy specifies `@pattern` as a *find* — the expression has to match SOMEWHERE in the
//! value — and the framework's request boundary implements exactly that
//! (`vendor/smithy-schema-server/src/constraints.rs`: "Smithy pattern semantics are a
//! *find* … which is exactly `regex::Regex::is_match`"). The service enforces the
//! expression over the whole value instead, so a value that satisfies it only in part is
//! refused before any handler work rather than carried into a lookup: for
//! `[0-9a-z-:]+`, an id spelled with a space or an uppercase letter beside a legal
//! character is *found* and so reaches the handler, which can only report it as a resource
//! that is not there — a different claim about a value that cannot name a resource at all.
//!
//! For an expression a model ANCHORS with `^` / `$`, and for one that admits every value a
//! find admits, the two readings coincide and this walk refuses exactly what the boundary
//! already refused. They differ for an expression that is a character class or a grammar
//! over the whole value, which is what this service's id and bucket shapes declare.
//!
//! NOTHING about pattern checking is reimplemented here, and no expression is written out:
//! each one is READ off a member's own generated schema, re-declared in the framework's own
//! form ([`PatternTrait`]) with the anchors that make it a whole-value requirement, and the
//! check is [`validate`] — the same walk the request boundary runs. The walk below visits
//! every string an operation's input carries and holds each to the expression ITS member
//! declares, so one statement of the rule serves every operation and every member.

use std::sync::LazyLock;

use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use aws_smithy_schema_server::constraints::{validate, ElementConstraintsTrait, PatternTrait};
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

use crate::ops::types::ValidationException;

/// This service's Smithy namespace, for the shadow schemas below.
const NAMESPACE: &str = "com.amazonaws.migrationhubstrategy";

/// Refuse `input` when any string it carries falls outside the `@pattern` its member
/// declares, read over the WHOLE value.
///
/// `schema` is the operation's own `INPUT_SCHEMA`, so the walk covers every member of that
/// operation — top-level, nested in a structure or a union, and each element of a list of
/// strings — and nothing else. `Ok` for an input whose every pattern-bearing member
/// matches, and for one that carries no such member at all.
///
/// The refusal is this service's one [`crate::state::invalid_request_body`] sentence, the
/// same one the boundary gives a value outside a bound it does enforce, so a caller sees
/// one refusal for one logical failure whichever reading of the expression decided it. Each
/// operation lifts it into its own error channel.
pub fn check_whole_value_patterns(
    input: &dyn SerializableStruct,
    schema: &Schema,
) -> Result<(), ValidationException> {
    let mut walk = WholeValuePatterns::default();
    // The root input is a struct; drive it the way the codec — and the framework's own
    // validating walk — does. Every method of this serializer answers `Ok`, so the walk is
    // total and the result carries nothing to read; `debug_assert` says so where a future
    // serializer change would make it false.
    let walked = walk.write_struct(schema, input);
    debug_assert!(walked.is_ok(), "the whole-value pattern walk cannot fail");
    if walk.outside {
        return Err(crate::state::invalid_request_body());
    }
    Ok(())
}

/// A `ShapeSerializer` that walks a built input and records whether any string it carries
/// falls outside its member's modelled `@pattern` as a whole-value requirement.
///
/// It accumulates one BIT, not a violation list: the refusal this service words carries
/// neither the member that failed nor the value it rejected
/// ([`crate::state::INVALID_REQUEST_BODY`]), so there is nothing a path or an echo would
/// be read for, and the walk stops as soon as one member fails.
#[derive(Debug, Default)]
struct WholeValuePatterns {
    /// Whether a string outside its member's pattern has been seen.
    outside: bool,
}

impl WholeValuePatterns {
    /// Check one string leaf against every `@pattern` its schema declares for it.
    ///
    /// Two places carry one: the schema's OWN trait map, for a string written as a struct
    /// member, and the [`ElementConstraintsTrait`] wrapper, for a scalar element of a LIST
    /// — codegen passes the collection member's schema for those, whose own trait map
    /// describes the collection (`@length` as an element count) rather than the element. A
    /// `@pattern` is only ever emitted for a string shape, so the two can never describe the
    /// same value and reading both needs no bookkeeping about where the leaf sits.
    ///
    /// A MAP member's wrapper is deliberately not read: its key and its value are written
    /// with that one schema, and the wrapper describes the value alone, so holding a key to
    /// it would refuse a key for the value shape's expression. No input member of this
    /// service is a map, and a check that cannot tell the two apart is better left not
    /// made than made of the wrong half.
    fn leaf(&mut self, schema: &Schema, value: &str) {
        if self.outside {
            return;
        }
        let own = schema.traits();
        let element = matches!(schema.shape_type(), ShapeType::List)
            .then(|| element_constraints(schema))
            .flatten();
        for traits in [own, element].into_iter().flatten() {
            if let Some(expression) = declared_pattern(traits) {
                if outside_whole_value(expression, value) {
                    self.outside = true;
                    return;
                }
            }
        }
    }
}

/// The `@pattern` expression a trait map declares, or `None` when it declares none.
fn declared_pattern(traits: &TraitMap) -> Option<&'static str> {
    let declared = traits.get(&PatternTrait::TRAIT_ID)?;
    Some(declared.as_any().downcast_ref::<PatternTrait>()?.regex)
}

/// The element/value shape's own constraint trait map, when a collection member's schema
/// carries the wrapper the framework puts it in.
fn element_constraints(schema: &Schema) -> Option<&TraitMap> {
    let wrapper = schema.traits()?.get(&ElementConstraintsTrait::TRAIT_ID)?;
    Some(
        wrapper
            .as_any()
            .downcast_ref::<ElementConstraintsTrait>()?
            .inner(),
    )
}

/// Whether the WHOLE of `value` falls outside the modelled `@pattern` `expression`.
///
/// The check is the framework's own [`validate`] over the shadow member whose trait map
/// carries that expression anchored ([`shadow_for`]), so a refusal decided here is the same
/// judgement the request boundary makes — only of the whole value rather than of a part of
/// it. An expression no shadow carries is not checked: the shadows span every expression
/// this service's models declare, and each member's refusal is pinned in
/// `tests/all/whole_value_patterns.rs`, so a model that grows a new expression fails those
/// rows rather than being refused by a rule nothing states.
fn outside_whole_value(expression: &str, value: &str) -> bool {
    let Some(member) = shadow_for(expression) else {
        return false;
    };
    !validate(&WholeValue { member, value }, &WHOLE_VALUE_SCHEMA).is_empty()
}

/// One string written for the validating walk the way the codec writes a string member,
/// against the shadow member carrying the anchored expression it is held to.
struct WholeValue<'a> {
    member: &'static Schema,
    value: &'a str,
}

impl SerializableStruct for WholeValue<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(self.member, self.value)
    }
}

/// The root of the shadow input the walk is driven over. Its member list is empty on
/// purpose: the walk visits what [`WholeValue`] WRITES, and a root struct schema carries no
/// member name, so it contributes no path segment.
static WHOLE_VALUE_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "com.amazonaws.migrationhubstrategy#WholeValue",
        NAMESPACE,
        "WholeValue",
    ),
    ShapeType::Structure,
    &[],
);

/// `expression` with the anchors that turn the find into a whole-value requirement, in a
/// non-capturing group so a top-level alternation stays inside them.
///
/// No `s` flag, so `.` matches no line terminator — deliberately, and it is what the
/// whole-value reading means for an expression built on `.`: `.*\S.*` matches a line of a
/// multi-line value and not the value, so such a value is outside the expression. The same
/// reading of the same class is what
/// `services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/src/state.rs`
/// records for `[^\n]*` ("including one carrying a line feed, which as a whole it does not
/// match"). Adding the flag would make this check accept a value the expression does not
/// match, which is the gap it exists to close.
fn anchored(expression: &str) -> String {
    format!("^(?:{expression})$")
}

/// The `@pattern` expression `member` of `input` declares, or `None` for a member that
/// declares none (or that the schema does not carry).
///
/// Public so a test can assert the model still declares an expression for each member the
/// shadows below are built from: `None` there would quietly stop the whole-value check for
/// every member sharing that expression.
pub fn modelled_member_pattern(input: &Schema, member: &str) -> Option<&'static str> {
    let member = input
        .members()
        .iter()
        .find(|candidate| candidate.member_name() == Some(member))?;
    declared_pattern(member.traits()?)
}

/// A string member carrying one modelled expression, anchored, in the framework's own form.
///
/// Declares the three statics the framework's walk needs for one expression: the expression
/// as the model declares it (read off `$input`'s `$member`, never written out here), that
/// expression anchored, and the member schema whose trait map carries it. The shape id is
/// one the model does not declare, so it cannot collide with a modelled shape, and the
/// member name is the one the refusal never quotes.
macro_rules! whole_value_shadow {
    (
        $(#[$doc:meta])*
        $expression:ident, $anchored:ident, $traits:ident, $member_schema:ident,
        $input:expr, $member:literal, $shadow:literal
    ) => {
        $(#[$doc])*
        static $expression: LazyLock<Option<&'static str>> =
            LazyLock::new(|| modelled_member_pattern($input, $member));

        #[doc = concat!("[`", stringify!($expression), "`] anchored over the whole value.")]
        static $anchored: LazyLock<Option<String>> =
            LazyLock::new(|| $expression.map(|declared| anchored(declared)));

        #[doc = concat!("[`", stringify!($anchored), "`] as the framework's own trait.")]
        static $traits: LazyLock<TraitMap> = LazyLock::new(|| {
            let mut traits = TraitMap::new();
            if let Some(expression) = $anchored.as_deref() {
                traits.insert(Box::new(PatternTrait::new(expression)));
            }
            traits
        });

        #[doc = concat!("The shadow member carrying [`", stringify!($traits), "`].")]
        static $member_schema: Schema = Schema::new_member(
            ShapeId::from_static(
                concat!("com.amazonaws.migrationhubstrategy#WholeValue$", $shadow),
                NAMESPACE,
                $shadow,
            ),
            ShapeType::String,
            $shadow,
            0,
        )
        .with_traits(&$traits);
    };
}

whole_value_shadow!(
    /// The expression this service's async-task ids declare (`AsyncTaskId`,
    /// `RecommendationTaskId`), read off `StopAssessmentRequest$assessmentId`.
    TASK_ID_EXPRESSION,
    TASK_ID_ANCHORED,
    TASK_ID_TRAITS,
    M_TASK_ID,
    crate::ops::types::STOP_ASSESSMENT_INPUT_SCHEMA,
    "assessmentId",
    "taskId"
);

whole_value_shadow!(
    /// The expression `ApplicationComponentId` declares, read off
    /// `GetApplicationComponentDetailsRequest$applicationComponentId`.
    COMPONENT_ID_EXPRESSION,
    COMPONENT_ID_ANCHORED,
    COMPONENT_ID_TRAITS,
    M_COMPONENT_ID,
    crate::ops::types::GET_APPLICATION_COMPONENT_DETAILS_INPUT_SCHEMA,
    "applicationComponentId",
    "componentId"
);

whole_value_shadow!(
    /// The expression this service's own `String` shape declares — the one every free-form
    /// string member of this model targets, from an id to a `nextToken` to a filter value —
    /// read off `GetImportFileTaskRequest$id`.
    TEXT_EXPRESSION,
    TEXT_ANCHORED,
    TEXT_TRAITS,
    M_TEXT,
    crate::ops::types::GET_IMPORT_FILE_TASK_INPUT_SCHEMA,
    "id",
    "text"
);

whole_value_shadow!(
    /// The expression this service's bucket members declare (`importS3Bucket`), read off
    /// `StartImportFileTaskRequest$S3Bucket`.
    BUCKET_EXPRESSION,
    BUCKET_ANCHORED,
    BUCKET_TRAITS,
    M_BUCKET,
    crate::ops::types::START_IMPORT_FILE_TASK_INPUT_SCHEMA,
    "S3Bucket",
    "bucket"
);

whole_value_shadow!(
    /// The expression `SecretsManagerKey` declares, read off
    /// `UpdateApplicationComponentConfigRequest$secretsManagerKey`.
    SECRET_EXPRESSION,
    SECRET_ANCHORED,
    SECRET_TRAITS,
    M_SECRET,
    crate::ops::types::UPDATE_APPLICATION_COMPONENT_CONFIG_INPUT_SCHEMA,
    "secretsManagerKey",
    "secret"
);

/// Every distinct `@pattern` this service's input members declare, paired with the shadow
/// member that holds a value to it over its whole length.
///
/// Keyed by the expression itself, so a member is covered by the shape it targets rather
/// than by being named here: every member targeting `AsyncTaskId` or `RecommendationTaskId`
/// is held to the first row, every member targeting this model's `String`, `NextToken`,
/// `ServerId`, `Location`, `ProjectName` or `SourceVersion` to the third, and so on. An
/// expression with no row is not checked, which is why each row's source is a member whose
/// own refusal is pinned in `tests/all/whole_value_patterns.rs`.
static SHADOWS: LazyLock<Vec<(&'static str, &'static Schema)>> = LazyLock::new(|| {
    [
        (*TASK_ID_EXPRESSION, &M_TASK_ID),
        (*COMPONENT_ID_EXPRESSION, &M_COMPONENT_ID),
        (*TEXT_EXPRESSION, &M_TEXT),
        (*BUCKET_EXPRESSION, &M_BUCKET),
        (*SECRET_EXPRESSION, &M_SECRET),
    ]
    .into_iter()
    .filter_map(|(expression, member)| expression.map(|expression| (expression, member)))
    .collect()
});

/// The shadow member carrying `expression` anchored, or `None` when no model member this
/// service reads declares it.
fn shadow_for(expression: &str) -> Option<&'static Schema> {
    SHADOWS
        .iter()
        .find(|(declared, _)| *declared == expression)
        .map(|(_, member)| *member)
}

/// Every expression the shadows above are built from, in declaration order — `None` for one
/// the model no longer declares.
///
/// Public for the test that asserts the model still declares each of them, so a renamed
/// member or a dropped `@pattern` fails a test rather than silently admitting every value.
pub fn shadowed_expressions() -> Vec<Option<&'static str>> {
    vec![
        *TASK_ID_EXPRESSION,
        *COMPONENT_ID_EXPRESSION,
        *TEXT_EXPRESSION,
        *BUCKET_EXPRESSION,
        *SECRET_EXPRESSION,
    ]
}

impl ShapeSerializer for WholeValuePatterns {
    fn write_struct(
        &mut self,
        _schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        if self.outside {
            return Ok(());
        }
        // A struct is the root input, a nested structure or union member, a list element or
        // a map value; each of its members writes itself with its own member schema, which
        // is where the `@pattern` is read from, so the walk needs no path of its own.
        value.serialize_members(self)
    }

    fn write_list(
        &mut self,
        _schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        if self.outside {
            return Ok(());
        }
        write_elements(self)
    }

    fn write_map(
        &mut self,
        _schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        if self.outside {
            return Ok(());
        }
        write_entries(self)
    }

    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        self.leaf(schema, value);
        Ok(())
    }

    // `@pattern` applies to strings alone (the generated schema emits it for no other
    // shape), so every other leaf is walked past rather than checked.
    fn write_boolean(&mut self, _schema: &Schema, _value: bool) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_byte(&mut self, _schema: &Schema, _value: i8) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_short(&mut self, _schema: &Schema, _value: i16) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_integer(&mut self, _schema: &Schema, _value: i32) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_long(&mut self, _schema: &Schema, _value: i64) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_float(&mut self, _schema: &Schema, _value: f32) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_double(&mut self, _schema: &Schema, _value: f64) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_big_integer(
        &mut self,
        _schema: &Schema,
        _value: &BigInteger,
    ) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_big_decimal(
        &mut self,
        _schema: &Schema,
        _value: &BigDecimal,
    ) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_blob(&mut self, _schema: &Schema, _value: &[u8]) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_timestamp(&mut self, _schema: &Schema, _value: &DateTime) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_document(&mut self, _schema: &Schema, _value: &Document) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_null(&mut self, _schema: &Schema) -> Result<(), SerdeError> {
        Ok(())
    }
}
