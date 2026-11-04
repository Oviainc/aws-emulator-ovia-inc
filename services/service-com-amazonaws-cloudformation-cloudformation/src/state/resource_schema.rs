// SPDX-License-Identifier: CC0-1.0

//! The RESOURCE SCHEMA the extension registry holds for a resource type: `Schema`, "the
//! schema that defines the extension", which `DescribeType` reports for the version it
//! describes.
//!
//! A resource type in the CloudFormation registry is defined by a JSON document — the
//! resource provider definition schema — and that document is the authority on which
//! properties the type has: it is what a client reads the registry to learn, and what
//! another service validating a payload against a resource type (AWS Config's
//! `PutResourceConfig` is one) resolves the type's contract from. A registry record whose
//! schema is absent cannot be told apart from a type that is not registered at all.
//!
//! The registry of every account and Region holds the public extensions Amazon Web
//! Services publishes without anybody registering them
//! ([`super::registry::aws_published`]), and for those this module is where the schema
//! comes from. It is DERIVED, not stored: a published type's record is not written by a
//! registry call, so there is no handler package anybody uploaded whose schema could have
//! been persisted. What this emulator knows about such a type is the type's own catalogue
//! entry — the properties a resource of it publishes, which
//! [`super::template::published_attribute_names`] carries straight out of that type's
//! "Return values" section, and the open-source repository its resource provider is built
//! from ([`super::registry::aws_published_source_url`]) — and the document carries exactly
//! that, under the member names the provider definition meta-schema fixes:
//!
//! * `typeName` — the type the registry record is for;
//! * `properties` — one entry per property the type publishes, nested where the published
//!   name is a path through one (`Endpoint.Address` is `Endpoint`'s `Address`), each typed
//!   `string` because every value this emulator reports for one is text;
//! * `sourceUrl` — the repository the type's resource provider is published in.
//!
//! What it deliberately does NOT carry is anything the emulator does not know:
//!
//! * no `additionalProperties: false`, which would state that the properties above are
//!   ALL the type accepts — the writable properties of an AWS-published type are not
//!   known here, so the document is explicitly open;
//! * no `readOnlyProperties`/`createOnlyProperties` classification, since a published
//!   return value can also be a property a template sets (a queue's `QueueName` is both)
//!   and nothing here tells the two apart;
//! * no `primaryIdentifier`, because which PROPERTY holds the physical id `Ref` returns
//!   is not something the return-value catalogue records;
//! * no `handlers`, because what a handler of one of these types requires — the permissions
//!   [`schema_handler_permissions`] reads back out of a schema that declares them — is not
//!   something the return-value catalogue records either;
//! * no `description`, which is prose about the type that this emulator holds nowhere —
//!   the same reason the record's own `Description` member stays absent.
//!
//! A type the catalogue carries nothing for gets NO schema rather than an empty document:
//! a record without a schema is what an extension whose schema this emulator does not hold
//! — a private extension registered from a handler package, or a public resource type no
//! resource provider stands behind, whose defining document is nowhere here — reports.

use ::serde_json::{json, Map, Value};

/// The meta-schema a CloudFormation resource provider definition declares itself against,
/// which is the `$schema` every published resource schema carries.
const PROVIDER_DEFINITION_META_SCHEMA: &str =
    "https://schema.cloudformation.us-east-1.amazonaws.com/provider.definition.schema.v1.json";

/// The resource schema of the resource type `type_name`, as the JSON text the registry
/// holds and `DescribeType` reports as `Schema`; `None` for a type this emulator carries
/// no published properties for, which is a type its registry holds no record for either.
pub fn resource_type_schema(type_name: &str) -> Option<String> {
    let published = super::template::published_attribute_names(type_name);
    if published.is_empty() {
        return None;
    }
    Some(document(type_name, published).to_string())
}

/// The schema document for `type_name`, built from the properties it publishes.
fn document(type_name: &str, published: &[&str]) -> Value {
    let mut properties = Map::new();
    for property in published {
        define_property(&mut properties, property);
    }
    let mut document = Map::new();
    document.insert(
        "$schema".to_string(),
        Value::String(PROVIDER_DEFINITION_META_SCHEMA.to_string()),
    );
    document.insert("typeName".to_string(), Value::String(type_name.to_string()));
    document.insert("properties".to_string(), Value::Object(properties));
    if let Some(source_url) = super::registry::aws_published_source_url(type_name) {
        document.insert("sourceUrl".to_string(), Value::String(source_url));
    }
    Value::Object(document)
}

/// The permissions the HANDLERS of an extension require, read out of the schema that defines
/// it — what `DescribeType` reports as `SchemaHandlerPermissions`.
///
/// A resource provider definition schema declares its handlers under `handlers`, one entry per
/// handler (`create`, `read`, `update`, `delete`, `list`), and each entry declares the
/// `permissions` that handler needs — "the permissions required for the handler to run"
/// (the provider definition meta-schema [`PROVIDER_DEFINITION_META_SCHEMA`] this registry
/// holds its schemas against). The permissions of an extension are therefore the union of
/// those lists, read off the schema the registry holds for the version being described rather
/// than held a second time beside it, and sorted so one document always reads back as the same
/// set.
///
/// The answer is a SET, possibly empty: an extension whose schema declares no handler — which
/// is every extension this registry derives a schema for ([`resource_type_schema`] documents
/// what such a document carries and what it deliberately does not) — and one the registry
/// holds no schema for at all both require no handler permission, which is what the empty set
/// says.
pub fn schema_handler_permissions(schema: Option<&str>) -> Vec<String> {
    let mut permissions: ::std::collections::BTreeSet<String> = ::std::collections::BTreeSet::new();
    for handler in handlers_object(schema).values() {
        let declared = handler
            .get("permissions")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for permission in declared {
            if let Some(permission) = permission.as_str() {
                permissions.insert(permission.to_string());
            }
        }
    }
    permissions.into_iter().collect()
}

/// The `handlers` member of a provider definition schema: one entry per handler the extension
/// declares (`create`, `read`, `update`, `delete`, `list`), each with what that handler
/// declares about itself. The ONE reading of that member, since both what the handlers REQUIRE
/// ([`schema_handler_permissions`]) and WHICH of them exist
/// ([`declared_provisioning_type`]) are read out of it.
///
/// Empty for a schema that declares no handler and for text that is no such document at all.
fn handlers_object(schema: Option<&str>) -> Map<String, Value> {
    schema
        .and_then(|schema| ::serde_json::from_str::<Value>(schema).ok())
        .as_ref()
        .and_then(|document| document.get("handlers"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The extension a provider definition schema DECLARES, under the `typeName` member the
/// meta-schema [`PROVIDER_DEFINITION_META_SCHEMA`] fixes — the member every schema this
/// registry holds carries ([`document`] writes it, and [`super::registry::type_tests_outcome`]
/// requires it of a version being tested).
///
/// `None` for text that is not such a document at all, which declares no extension.
pub fn declared_type_name(schema: &str) -> Option<String> {
    ::serde_json::from_str::<Value>(schema)
        .ok()?
        .get("typeName")?
        .as_str()
        .map(str::to_string)
}

/// The `description` a provider definition schema carries — prose about the extension the
/// document defines, which is what `DescribeType` reports as the version's `Description`.
///
/// `None` where the document carries none, since the registry holds no description of its own
/// for a version beside the one its schema states.
pub fn declared_description(schema: &str) -> Option<String> {
    ::serde_json::from_str::<Value>(schema)
        .ok()?
        .get("description")?
        .as_str()
        .map(str::to_string)
}

/// The handlers a provider definition schema declares, by name (`create`, `read`, `update`,
/// `delete`, `list`): the keys of the same `handlers` member
/// [`schema_handler_permissions`] reads the permissions out of.
fn declared_handlers(schema: Option<&str>) -> ::std::collections::BTreeSet<String> {
    handlers_object(schema).keys().cloned().collect()
}

/// The provisioning behaviour a REGISTRATION determines for the resource type it registers:
/// "CloudFormation determines the provisioning type during registration, based on the types
/// of handlers in the schema handler package submitted"
/// (`smithy_query(query="doc", shape="DescribeTypeOutput", member="ProvisioningType")`).
///
/// That member states the rule handler by handler, and this is it: a type that does not
/// declare all of `create`, `read` and `delete` "can't actually be provisioned"
/// (`NON_PROVISIONABLE`); one that declares those and an `update` handler "includes an update
/// handler to process updates to the type during stack update operations"
/// (`FULLY_MUTABLE`); one that declares them without an `update` handler "can't be updated
/// and must instead be replaced during stack update operations" (`IMMUTABLE`).
///
/// A package carrying no schema for the extension declares no handler, so it is
/// `NON_PROVISIONABLE` — what a registration can say about a type whose handlers are nowhere
/// to be read.
pub fn declared_provisioning_type(schema: Option<&str>) -> &'static str {
    use super::registry::provisioning_type;

    let handlers = declared_handlers(schema);
    if !PROVISIONING_HANDLERS
        .iter()
        .all(|required| handlers.contains(*required))
    {
        return provisioning_type::NON_PROVISIONABLE;
    }
    if handlers.contains(UPDATE_HANDLER) {
        provisioning_type::FULLY_MUTABLE
    } else {
        provisioning_type::IMMUTABLE
    }
}

/// The handlers a resource type must declare to be provisionable at all: without all of them
/// the type "doesn't include all the following handlers, and therefore can't actually be
/// provisioned: create, read, delete".
const PROVISIONING_HANDLERS: &[&str] = &["create", "read", "delete"];

/// The handler whose presence separates `FULLY_MUTABLE` from `IMMUTABLE`.
const UPDATE_HANDLER: &str = "update";

/// Define one published property under `into`.
///
/// A published name that is a PATH through a property (`Endpoint.Address`, which is the
/// `Address` of the resource's `Endpoint`) defines the property it passes through as an
/// object and recurses into it, so the document's shape is the shape of the value rather
/// than a flat name carrying a dot. A path through a property already defined as a scalar
/// — which no catalogued type has, since a type publishing `X` and `X.Y` would be
/// publishing one name as both — leaves that definition alone rather than overwriting it.
fn define_property(into: &mut Map<String, Value>, path: &str) {
    let Some((property, rest)) = path.split_once('.') else {
        into.entry(path.to_string())
            .or_insert_with(|| json!({"type": "string"}));
        return;
    };
    let nested = into
        .entry(property.to_string())
        .or_insert_with(|| json!({"type": "object", "properties": {}}));
    if let Some(members) = nested.get_mut("properties").and_then(Value::as_object_mut) {
        define_property(members, rest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every type the registry holds a published record for has a schema, and that schema
    /// is a JSON document naming that type and defining exactly the properties the type
    /// publishes.
    #[test]
    fn every_published_resource_type_has_a_schema_of_its_published_properties() {
        for type_name in super::super::template::IMPLEMENTED_RESOURCE_TYPES {
            let schema = resource_type_schema(type_name).unwrap_or_else(|| {
                panic!("case {type_name:?}: expected a schema, got none")
            });
            let document: Value = ::serde_json::from_str(&schema).unwrap_or_else(|error| {
                panic!("case {type_name:?}: expected JSON, got {schema:?} ({error})")
            });
            assert_eq!(
                document.get("typeName").and_then(Value::as_str),
                Some(*type_name),
                "case {type_name:?}: expected the schema to name its own type, got \
                 {document:?}"
            );
            // Each published name, as a JSON pointer into the document's properties: a
            // name that is a path through a property points through its `properties`.
            for published in super::super::template::published_attribute_names(type_name) {
                let pointer = format!(
                    "/properties/{}",
                    published.split('.').collect::<Vec<_>>().join("/properties/")
                );
                let defined = document.pointer(&pointer);
                assert!(
                    defined.is_some(),
                    "case {type_name:?}: expected {pointer:?} to be defined, got \
                     {document:?}"
                );
            }
        }
    }

    /// A type whose properties this emulator knows nothing about gets no schema at all,
    /// rather than a document defining no property.
    #[test]
    fn an_uncatalogued_type_has_no_schema() {
        let schema = resource_type_schema("AWS::NotAService::NotAType");
        assert!(
            schema.is_none(),
            "case \"a type no catalogue entry carries\": expected no schema, got \
             {schema:?}"
        );
    }

    /// A published name that is a path through a property defines that property as an
    /// object carrying the member the path names, so the document's shape is the value's.
    #[test]
    fn a_dotted_published_name_defines_a_nested_property() {
        let document = document(
            "AWS::RDS::DBInstance",
            &["Endpoint.Address", "Endpoint.Port"],
        );
        for (case, pointer, expected) in [
            ("the property the path passes through", "/properties/Endpoint/type", "object"),
            ("the first member", "/properties/Endpoint/properties/Address/type", "string"),
            ("the second member", "/properties/Endpoint/properties/Port/type", "string"),
        ] {
            let actual = document.pointer(pointer).and_then(Value::as_str);
            assert_eq!(
                actual,
                Some(expected),
                "case {case:?}: expected {pointer:?} to be {expected:?}, got {actual:?} \
                 in {document:?}"
            );
        }
    }
}
