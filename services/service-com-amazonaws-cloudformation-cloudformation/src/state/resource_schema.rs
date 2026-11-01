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
//! the registry holds no record for such a type at all ([`super::registry::aws_published`]
//! answers `None` outside [`super::template::IMPLEMENTED_RESOURCE_TYPES`]), and a record
//! without a schema is what an extension whose schema this emulator does not hold — a
//! private extension registered from a handler package, say — reports.

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
    let handlers = schema
        .and_then(|schema| ::serde_json::from_str::<Value>(schema).ok())
        .as_ref()
        .and_then(|document| document.get("handlers"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for handler in handlers.values() {
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
