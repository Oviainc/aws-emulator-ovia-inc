// SPDX-License-Identifier: CC0-1.0

//! The GraphQL types a GraphQL API holds: how one caller-supplied type definition is
//! parsed, kept and reported in either format the model names.
//!
//! A type arrives as "The type definition, in GraphQL Schema Definition Language (SDL)
//! format" (`CreateTypeRequest$definition`), so a stored type keeps the SDL text the
//! caller wrote together with the definition that text parses to. The
//! `TypeDefinitionFormat` a call names then chooses the REPRESENTATION the reported
//! `Type$definition` carries — the SDL the type was defined with, or the type's
//! introspection JSON — and that JSON is computed from the definitions the API holds,
//! never from the request.
//!
//! The parse is a real GraphQL parse (`apollo-parser`), so what the service accepts is
//! what GraphQL's SDL grammar accepts, and a definition it cannot parse is refused
//! rather than stored as text nothing understands.

use std::collections::BTreeMap;

use apollo_parser::cst::{self, CstNode};
use apollo_parser::Parser;
use serde_json::{json, Value};

use crate::ops::types::Type;

/// The `TypeDefinitionFormat` value naming a definition written in GraphQL SDL.
pub const SDL_FORMAT: &str = "SDL";

/// The `TypeDefinitionFormat` value naming a definition reported as the type's
/// introspection JSON.
pub const JSON_FORMAT: &str = "JSON";

/// The deepest nesting the GraphQL parser descends into one caller-supplied type
/// definition, and the most tokens it reads from one. `CreateTypeRequest$definition`
/// and `UpdateTypeRequest$definition` carry no modelled `@length`, so without these a
/// single request chooses how much stack and CPU one parse costs; past either bound the
/// parse fails and the request is refused.
pub const DEFINITION_RECURSION_LIMIT: usize = 64;

/// See [`DEFINITION_RECURSION_LIMIT`].
pub const DEFINITION_TOKEN_LIMIT: usize = 100_000;

/// The `__TypeKind` an SDL `type` definition has, as GraphQL's introspection names it.
pub const OBJECT_KIND: &str = "OBJECT";

/// The `__TypeKind` an SDL `interface` definition has.
pub const INTERFACE_KIND: &str = "INTERFACE";

/// The `__TypeKind` an SDL `input` definition has.
pub const INPUT_OBJECT_KIND: &str = "INPUT_OBJECT";

/// The `__TypeKind` an SDL `enum` definition has.
pub const ENUM_KIND: &str = "ENUM";

/// The `__TypeKind` an SDL `union` definition has.
pub const UNION_KIND: &str = "UNION";

/// The `__TypeKind` an SDL `scalar` definition has.
pub const SCALAR_KIND: &str = "SCALAR";

/// The `__TypeKind` a list type REFERENCE has (`[Post]`).
pub const LIST_KIND: &str = "LIST";

/// The `__TypeKind` a non-null type REFERENCE has (`Post!`).
pub const NON_NULL_KIND: &str = "NON_NULL";

/// The scalar types every AppSync schema holds without defining them: GraphQL's five
/// built-in scalars (`Int`, `Float`, `String`, `Boolean`, `ID`) and the nine AppSync
/// adds (`AWSDate`, `AWSTime`, `AWSDateTime`, `AWSTimestamp`, `AWSEmail`, `AWSJSON`,
/// `AWSURL`, `AWSPhone`, `AWSIPAddress`).
///
/// A reference to one of these names resolves to a [`SCALAR_KIND`] even though no
/// `CreateType` call defined it, which is what lets a field's type reference carry its
/// kind without the schema holding a definition for it.
pub const BUILT_IN_SCALARS: &[&str] = &[
    "Int",
    "Float",
    "String",
    "Boolean",
    "ID",
    "AWSDate",
    "AWSTime",
    "AWSDateTime",
    "AWSTimestamp",
    "AWSEmail",
    "AWSJSON",
    "AWSURL",
    "AWSPhone",
    "AWSIPAddress",
];

/// The directive a field or enum value is marked deprecated with.
const DEPRECATED_DIRECTIVE: &str = "deprecated";

/// The argument [`DEPRECATED_DIRECTIVE`] states its reason in.
const DEPRECATED_REASON_ARGUMENT: &str = "reason";

/// The reason introspection reports for a deprecation whose directive names none:
/// `@deprecated`'s `reason` argument has exactly this default in GraphQL's own
/// definition of the directive.
const DEFAULT_DEPRECATION_REASON: &str = "No longer supported";

/// One stored GraphQL type of a `GraphqlApiRecord`.
///
/// Keeps the definition as the caller wrote it AND the definition it parses to: the
/// first is what an SDL report answers with, the second is what a JSON report is
/// rendered from, so the two representations of one type cannot disagree.
#[derive(Debug, Clone)]
pub struct TypeRecord {
    /// Creation order, used to keep a listing (and its `nextToken` cursor) stable
    /// across calls.
    pub seq: u64,
    /// The type's ARN, named under its API's own path.
    pub arn: String,
    /// The SDL text the create (or the last update) carried, kept verbatim.
    pub definition: String,
    /// What [`Self::definition`] parses to.
    pub parsed: TypeDefinition,
}

impl TypeRecord {
    /// This record as the modelled `Type` shape, with its definition in `format`.
    ///
    /// The single projection every operation that reports a type answers with, so a
    /// create and the reads after it cannot disagree about one type. `api_types` is the
    /// set of types the type's own API holds, which the JSON rendering resolves type
    /// references against.
    pub fn to_type(&self, format: &str, api_types: &BTreeMap<String, TypeRecord>) -> Type {
        Type {
            name: Some(self.parsed.name.clone()),
            description: self.parsed.description.clone(),
            arn: Some(self.arn.clone()),
            definition: Some(self.definition_in(format, api_types)),
            format: Some(format.to_string()),
        }
    }

    /// This type's definition in `format`: the stored SDL, or the type's introspection
    /// JSON rendered from the stored definitions.
    fn definition_in(&self, format: &str, api_types: &BTreeMap<String, TypeRecord>) -> String {
        if format == JSON_FORMAT {
            serde_json::to_string_pretty(&self.introspection(api_types)).unwrap_or_default()
        } else {
            self.definition.clone()
        }
    }

    /// This type as a GraphQL introspection `__Type`: the JSON representation of a
    /// GraphQL type, as the introspection system defines it.
    ///
    /// Every member is taken from the stored definition, and every type REFERENCE is
    /// resolved against the types the API holds plus the schema's built-in scalars
    /// ([`BUILT_IN_SCALARS`]) — so an interface reports the object types that implement
    /// it, and a reference to a type nothing has defined carries its name with no kind
    /// rather than a kind the schema does not support.
    fn introspection(&self, api_types: &BTreeMap<String, TypeRecord>) -> Value {
        let parsed = &self.parsed;
        let has_fields = parsed.kind == OBJECT_KIND || parsed.kind == INTERFACE_KIND;
        json!({
            "kind": parsed.kind,
            "name": parsed.name,
            "description": parsed.description,
            "fields": has_fields.then(|| {
                parsed
                    .fields
                    .iter()
                    .map(|field| field_json(field, api_types))
                    .collect::<Vec<Value>>()
            }),
            "inputFields": (parsed.kind == INPUT_OBJECT_KIND).then(|| {
                parsed
                    .input_fields
                    .iter()
                    .map(|input| input_value_json(input, api_types))
                    .collect::<Vec<Value>>()
            }),
            "interfaces": has_fields.then(|| {
                parsed
                    .interfaces
                    .iter()
                    .map(|name| named_type_json(name, api_types))
                    .collect::<Vec<Value>>()
            }),
            "enumValues": (parsed.kind == ENUM_KIND).then(|| {
                parsed
                    .enum_values
                    .iter()
                    .map(enum_value_json)
                    .collect::<Vec<Value>>()
            }),
            "possibleTypes": self.possible_types(api_types),
        })
    }

    /// The types this one can resolve to at runtime: a union's members, and the object
    /// types that implement an interface. Every other kind has none, which
    /// introspection reports as null rather than as an empty list.
    fn possible_types(&self, api_types: &BTreeMap<String, TypeRecord>) -> Value {
        match self.parsed.kind {
            UNION_KIND => Value::Array(
                self.parsed
                    .members
                    .iter()
                    .map(|name| named_type_json(name, api_types))
                    .collect(),
            ),
            INTERFACE_KIND => Value::Array(
                api_types
                    .values()
                    .filter(|record| {
                        record.parsed.kind == OBJECT_KIND
                            && record.parsed.interfaces.contains(&self.parsed.name)
                    })
                    .map(|record| named_type_json(&record.parsed.name, api_types))
                    .collect(),
            ),
            _ => Value::Null,
        }
    }
}

/// One parsed GraphQL type definition: the type's kind, its name and the members that
/// kind carries.
///
/// Only the members the kind can hold are filled, so a projection reads them by kind
/// rather than guessing from what is empty.
#[derive(Debug, Clone, Default)]
pub struct TypeDefinition {
    /// The `__TypeKind` the SDL keyword declares ([`OBJECT_KIND`] and the rest).
    pub kind: &'static str,
    pub name: String,
    /// The definition's description, i.e. the SDL description string written in front
    /// of it, which is what a GraphQL type's description is.
    pub description: Option<String>,
    /// The interfaces an object or interface type implements.
    pub interfaces: Vec<String>,
    /// The fields an object or interface type declares.
    pub fields: Vec<FieldDefinition>,
    /// The input fields an input object type declares.
    pub input_fields: Vec<InputValueDefinition>,
    /// The values an enum type declares.
    pub enum_values: Vec<EnumValueDefinition>,
    /// The member types a union type names.
    pub members: Vec<String>,
}

/// One field of a parsed object or interface type.
#[derive(Debug, Clone)]
pub struct FieldDefinition {
    pub name: String,
    pub description: Option<String>,
    pub arguments: Vec<InputValueDefinition>,
    /// The type the field resolves to, or `None` for a field whose definition names
    /// none.
    pub value_type: Option<TypeRef>,
    pub deprecation: Deprecation,
}

/// One argument of a field, or one field of an input object type: the two are the same
/// shape in GraphQL (`__InputValue`) and in SDL.
#[derive(Debug, Clone)]
pub struct InputValueDefinition {
    pub name: String,
    pub description: Option<String>,
    pub value_type: Option<TypeRef>,
    /// The default value as GraphQL writes it, which is the form introspection reports
    /// it in ("a GraphQL-formatted string representing the default value").
    pub default_value: Option<String>,
}

/// One value of a parsed enum type.
#[derive(Debug, Clone)]
pub struct EnumValueDefinition {
    pub name: String,
    pub description: Option<String>,
    pub deprecation: Deprecation,
}

/// Whether a field or enum value is deprecated, and why.
#[derive(Debug, Clone, Default)]
pub struct Deprecation {
    pub is_deprecated: bool,
    pub reason: Option<String>,
}

/// A reference to a type from a field or an argument: a name, a list of them, or a
/// non-null one.
#[derive(Debug, Clone)]
pub enum TypeRef {
    Named(String),
    List(Box<TypeRef>),
    NonNull(Box<TypeRef>),
}

/// Why a caller-supplied type definition is not one type definition this service can
/// store.
#[derive(Debug, Clone)]
pub enum TypeDefinitionError {
    /// The definition is not well-formed GraphQL, carrying the parser's own account of
    /// the first thing wrong with it.
    Unparseable(String),
    /// The definition parses but declares no type (an empty document, or one that only
    /// declares something a type cannot be, such as an operation or a directive).
    NoTypeDeclared,
    /// The definition declares more than one type, and `CreateType` "Creates a Type
    /// object" — one — so there is no single type for the call to answer with.
    MoreThanOneTypeDeclared,
}

impl TypeDefinitionError {
    /// The text a `BadRequestException` (HTTP 400) carries for this refusal: what about
    /// the definition the caller has to fix.
    ///
    /// The requirement the refusal answers is the status and the error shape; the
    /// wording follows the service's other malformed-request sentences, with the
    /// parser's account of a definition it could not read carried through so the caller
    /// can fix it from the message alone.
    pub fn message(&self) -> String {
        match self {
            Self::Unparseable(detail) => format!("Invalid type definition: {detail}"),
            Self::NoTypeDeclared => {
                "Invalid type definition: it must declare one GraphQL type.".to_string()
            }
            Self::MoreThanOneTypeDeclared => {
                "Invalid type definition: it must declare one GraphQL type, not several."
                    .to_string()
            }
        }
    }
}

/// Parse one caller-supplied type definition, written in GraphQL SDL.
///
/// A type definition declares exactly one type — `CreateType` "Creates a Type object"
/// and addresses it by that type's name afterwards — so a document that declares none,
/// or more than one, is refused rather than stored.
///
/// The name a definition declares is a GraphQL name (`[_A-Za-z][_0-9A-Za-z]*`), which
/// is the alphabet the modelled `ResourceName` admits, so a parsed name can always be
/// addressed back by `GetType`, `UpdateType` and `DeleteType`.
///
/// The parse is bounded by [`DEFINITION_RECURSION_LIMIT`] and
/// [`DEFINITION_TOKEN_LIMIT`], so neither the depth nor the amount of work one
/// caller-supplied definition costs comes from the request.
pub fn parse_type_definition(definition: &str) -> Result<TypeDefinition, TypeDefinitionError> {
    let tree = Parser::new(definition)
        .recursion_limit(DEFINITION_RECURSION_LIMIT)
        .token_limit(DEFINITION_TOKEN_LIMIT)
        .parse();
    if let Some(error) = tree.errors().next() {
        return Err(TypeDefinitionError::Unparseable(error.message().to_string()));
    }

    let mut declared: Option<TypeDefinition> = None;
    for node in tree.document().definitions() {
        let parsed = match node {
            cst::Definition::ObjectTypeDefinition(object) => TypeDefinition {
                kind: OBJECT_KIND,
                name: name_text(object.name())?,
                description: description_text(object.description()),
                interfaces: implemented_interfaces(object.implements_interfaces()),
                fields: field_definitions(object.fields_definition()),
                ..TypeDefinition::default()
            },
            cst::Definition::InterfaceTypeDefinition(interface) => TypeDefinition {
                kind: INTERFACE_KIND,
                name: name_text(interface.name())?,
                description: description_text(interface.description()),
                interfaces: implemented_interfaces(interface.implements_interfaces()),
                fields: field_definitions(interface.fields_definition()),
                ..TypeDefinition::default()
            },
            cst::Definition::InputObjectTypeDefinition(input) => TypeDefinition {
                kind: INPUT_OBJECT_KIND,
                name: name_text(input.name())?,
                description: description_text(input.description()),
                input_fields: input
                    .input_fields_definition()
                    .map(|fields| {
                        fields
                            .input_value_definitions()
                            .map(input_value_definition)
                            .collect()
                    })
                    .unwrap_or_default(),
                ..TypeDefinition::default()
            },
            cst::Definition::EnumTypeDefinition(enumeration) => TypeDefinition {
                kind: ENUM_KIND,
                name: name_text(enumeration.name())?,
                description: description_text(enumeration.description()),
                enum_values: enumeration
                    .enum_values_definition()
                    .map(|values| {
                        values
                            .enum_value_definitions()
                            .map(enum_value_definition)
                            .collect()
                    })
                    .unwrap_or_default(),
                ..TypeDefinition::default()
            },
            cst::Definition::UnionTypeDefinition(union) => TypeDefinition {
                kind: UNION_KIND,
                name: name_text(union.name())?,
                description: description_text(union.description()),
                members: union
                    .union_member_types()
                    .map(|members| named_type_names(members.named_types()))
                    .unwrap_or_default(),
                ..TypeDefinition::default()
            },
            cst::Definition::ScalarTypeDefinition(scalar) => TypeDefinition {
                kind: SCALAR_KIND,
                name: name_text(scalar.name())?,
                description: description_text(scalar.description()),
                ..TypeDefinition::default()
            },
            // Everything else a GraphQL document can declare — an operation, a
            // fragment, a directive, a schema, a type EXTENSION — declares no type, so
            // it leaves nothing for this call to create.
            _ => return Err(TypeDefinitionError::NoTypeDeclared),
        };
        if declared.replace(parsed).is_some() {
            return Err(TypeDefinitionError::MoreThanOneTypeDeclared);
        }
    }

    declared.ok_or(TypeDefinitionError::NoTypeDeclared)
}

/// The text of a declared name, or [`TypeDefinitionError::NoTypeDeclared`] for a
/// definition that names nothing.
fn name_text(name: Option<cst::Name>) -> Result<String, TypeDefinitionError> {
    name.map(|name| name.text().to_string())
        .filter(|name| !name.is_empty())
        .ok_or(TypeDefinitionError::NoTypeDeclared)
}

/// The text of an SDL description, which is a string value written in front of the
/// definition it describes.
fn description_text(description: Option<cst::Description>) -> Option<String> {
    description
        .and_then(|description| description.string_value())
        .map(String::from)
}

/// The names an `implements` clause lists.
fn implemented_interfaces(interfaces: Option<cst::ImplementsInterfaces>) -> Vec<String> {
    interfaces
        .map(|interfaces| named_type_names(interfaces.named_types()))
        .unwrap_or_default()
}

/// The names of a list of named type references.
fn named_type_names(types: impl Iterator<Item = cst::NamedType>) -> Vec<String> {
    types
        .filter_map(|named| named.name().map(|name| name.text().to_string()))
        .collect()
}

/// The fields a `{ … }` field set declares, in the order the definition writes them —
/// the order introspection reports them in.
fn field_definitions(fields: Option<cst::FieldsDefinition>) -> Vec<FieldDefinition> {
    fields
        .map(|fields| {
            fields
                .field_definitions()
                .map(|field| FieldDefinition {
                    name: field
                        .name()
                        .map(|name| name.text().to_string())
                        .unwrap_or_default(),
                    description: description_text(field.description()),
                    arguments: field
                        .arguments_definition()
                        .map(|arguments| {
                            arguments
                                .input_value_definitions()
                                .map(input_value_definition)
                                .collect()
                        })
                        .unwrap_or_default(),
                    value_type: type_reference(field.ty()),
                    deprecation: deprecation(field.directives()),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One argument or input field as its parsed form.
fn input_value_definition(input: cst::InputValueDefinition) -> InputValueDefinition {
    InputValueDefinition {
        name: input
            .name()
            .map(|name| name.text().to_string())
            .unwrap_or_default(),
        description: description_text(input.description()),
        value_type: type_reference(input.ty()),
        default_value: input
            .default_value()
            .and_then(|default| default.value())
            .map(|value| value.syntax().text().to_string().trim().to_string()),
    }
}

/// One enum value as its parsed form.
fn enum_value_definition(value: cst::EnumValueDefinition) -> EnumValueDefinition {
    EnumValueDefinition {
        name: value
            .enum_value()
            .and_then(|value| value.name())
            .map(|name| name.text().to_string())
            .unwrap_or_default(),
        description: description_text(value.description()),
        deprecation: deprecation(value.directives()),
    }
}

/// The type a field or argument names, as a reference tree.
fn type_reference(value_type: Option<cst::Type>) -> Option<TypeRef> {
    match value_type? {
        cst::Type::NamedType(named) => Some(TypeRef::Named(
            named.name().map(|name| name.text().to_string())?,
        )),
        cst::Type::ListType(list) => Some(TypeRef::List(Box::new(type_reference(list.ty())?))),
        cst::Type::NonNullType(non_null) => {
            let inner = match (non_null.named_type(), non_null.list_type()) {
                (Some(named), _) => {
                    TypeRef::Named(named.name().map(|name| name.text().to_string())?)
                }
                (_, Some(list)) => TypeRef::List(Box::new(type_reference(list.ty())?)),
                (None, None) => return None,
            };
            Some(TypeRef::NonNull(Box::new(inner)))
        }
    }
}

/// Whether the directives a field or enum value carries deprecate it, and the reason
/// they state.
fn deprecation(directives: Option<cst::Directives>) -> Deprecation {
    let Some(directives) = directives else {
        return Deprecation::default();
    };
    for directive in directives.directives() {
        if directive
            .name()
            .map(|name| name.text().to_string())
            .is_none_or(|name| name != DEPRECATED_DIRECTIVE)
        {
            continue;
        }
        let reason = directive
            .arguments()
            .and_then(|arguments| {
                arguments
                    .arguments()
                    .filter(|argument| {
                        argument
                            .name()
                            .is_some_and(|name| name.text() == DEPRECATED_REASON_ARGUMENT)
                    })
                    .find_map(|argument| match argument.value() {
                        Some(cst::Value::StringValue(reason)) => Some(String::from(reason)),
                        _ => None,
                    })
            })
            .unwrap_or_else(|| DEFAULT_DEPRECATION_REASON.to_string());
        return Deprecation {
            is_deprecated: true,
            reason: Some(reason),
        };
    }
    Deprecation::default()
}

/// One field as introspection's `__Field`.
fn field_json(field: &FieldDefinition, api_types: &BTreeMap<String, TypeRecord>) -> Value {
    json!({
        "name": field.name,
        "description": field.description,
        "args": field
            .arguments
            .iter()
            .map(|argument| input_value_json(argument, api_types))
            .collect::<Vec<Value>>(),
        "type": type_reference_json(field.value_type.as_ref(), api_types),
        "isDeprecated": field.deprecation.is_deprecated,
        "deprecationReason": field.deprecation.reason,
    })
}

/// One argument or input field as introspection's `__InputValue`.
fn input_value_json(
    input: &InputValueDefinition,
    api_types: &BTreeMap<String, TypeRecord>,
) -> Value {
    json!({
        "name": input.name,
        "description": input.description,
        "type": type_reference_json(input.value_type.as_ref(), api_types),
        "defaultValue": input.default_value,
    })
}

/// One enum value as introspection's `__EnumValue`.
fn enum_value_json(value: &EnumValueDefinition) -> Value {
    json!({
        "name": value.name,
        "description": value.description,
        "isDeprecated": value.deprecation.is_deprecated,
        "deprecationReason": value.deprecation.reason,
    })
}

/// A type reference as introspection's wrapping `__Type`: a list and a non-null
/// reference carry their inner reference in `ofType` and no name of their own.
fn type_reference_json(
    value_type: Option<&TypeRef>,
    api_types: &BTreeMap<String, TypeRecord>,
) -> Value {
    match value_type {
        None => Value::Null,
        Some(TypeRef::Named(name)) => named_type_json(name, api_types),
        Some(TypeRef::List(inner)) => json!({
            "kind": LIST_KIND,
            "name": Value::Null,
            "ofType": type_reference_json(Some(inner.as_ref()), api_types),
        }),
        Some(TypeRef::NonNull(inner)) => json!({
            "kind": NON_NULL_KIND,
            "name": Value::Null,
            "ofType": type_reference_json(Some(inner.as_ref()), api_types),
        }),
    }
}

/// A named type reference as introspection's `__Type`, with the kind the API's stored
/// types give that name.
fn named_type_json(name: &str, api_types: &BTreeMap<String, TypeRecord>) -> Value {
    json!({
        "kind": kind_of(name, api_types),
        "name": name,
        "ofType": Value::Null,
    })
}

/// The kind the type `name` has in this API: the kind of its stored definition, or
/// [`SCALAR_KIND`] for a scalar every schema holds ([`BUILT_IN_SCALARS`]). `None` when
/// nothing defines it, which is reported as a reference with no kind rather than as a
/// guess.
fn kind_of(name: &str, api_types: &BTreeMap<String, TypeRecord>) -> Option<&'static str> {
    if BUILT_IN_SCALARS.contains(&name) {
        return Some(SCALAR_KIND);
    }
    api_types.get(name).map(|record| record.parsed.kind)
}

/// The resource portion of a type's ARN: a type is a child of its API, so it is named
/// under the API's own `apis/{apiId}` path, as `GetType`'s URI
/// (`/v1/apis/{apiId}/types/{typeName}`) addresses it.
pub fn type_resource(api_id: &str, type_name: &str) -> String {
    format!("apis/{api_id}/types/{type_name}")
}

/// The text a `NotFoundException` (HTTP 404) carries for a type the named API does not
/// hold: the absent type, which is what the caller has to act on.
///
/// Shared by every operation that addresses a type by name, so they cannot disagree
/// about how a missing type is reported. Worded after the service's other
/// absent-resource sentences (`crate::state::data_source_not_found_message`); the
/// requirement it answers is the 404 and the `NotFoundException`.
pub fn type_not_found_message(type_name: &str) -> String {
    format!("Type {type_name} not found")
}

/// The text a `BadRequestException` (HTTP 400) carries for a create whose type name is
/// already taken within the API: the name, which is the member the caller has to
/// change.
///
/// Worded after the service's other name-taken sentence
/// (`crate::state::data_source_exists_message`).
pub fn type_exists_message(type_name: &str) -> String {
    format!("Type with name {type_name} already exists.")
}
