// SPDX-License-Identifier: CC0-1.0

//! The wire form of the tag entries a tag MUTATION reports back.
//!
//! WHY THIS MODULE EXISTS. The two tag mutations report what they changed —
//! `TagResourceResponse$tagsCreated` and `UntagResourceResponse$deletedTags`, both
//! `TagList` of `com.amazonaws.ram#Tag` — and RAM serializes those entries as
//! SELF-DESCRIBING JSON documents: each element carries the AWS JSON structure
//! discriminator `__type`, whose value is the serialized structure's own local name
//! (`Tag`), beside the modelled `key`/`value` pair. A tag reported as part of a
//! resource's STATE (`ResourceShare$tags`, `ResourceSharePermissionSummary$tags`,
//! `ResourceSharePermissionDetail$tags`) is a plain modelled structure and carries no
//! discriminator, so the discriminator belongs to these two members and not to the
//! `Tag` shape itself.
//!
//! `smithy_query(query="shape",shape="Tag",model_path="services/service-com-amazonaws-ram-amazonresourcesharing/smithy-model/com-amazonaws-ram-amazonresourcesharing.json")`
//! declares `com.amazonaws.ram#Tag` with exactly `key` and `value`, and
//! `#[smithy_macros::smithy(shape = ...)]` emits one member schema per modelled member
//! and nothing else, so a macro-built serializer cannot write a member the model does
//! not declare. `vendor/` is read-only from inside the emulator, so the two response
//! shapes state their own schema and serializer here: exactly what the attribute emits
//! for a one-member output struct, with the element writer pointed at
//! [`DocumentTag`] instead of at the bare `Tag`.
//!
//! WHAT IS DELIBERATELY ABSENT. `DeserializableStruct`: both structures are operation
//! OUTPUTS, which the server only ever serializes. The `@http` chain: it is baked onto
//! a shape only when the shape is an operation's INPUT. Constraint traits: the
//! validating walk runs on inputs only, so the attribute's captured constraints would
//! be inert on these shapes too.

use super::*;

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType};

/// This service's Smithy namespace, as every shape id below spells it.
const NAMESPACE: &str = "com.amazonaws.ram";

/// The value the discriminator carries on a tag entry: the local name of the
/// structure being serialized, read off the `Tag` shape's own schema rather than
/// spelled out, so it cannot drift from the shape.
fn tag_structure_name() -> &'static str {
    Tag::SCHEMA.shape_id().shape_name()
}

/// The member key the AWS JSON structure discriminator travels under.
const DISCRIMINATOR_MEMBER: &str = "__type";

/// The discriminator member of a `Tag` written as a document. Not a modelled member,
/// so it is indexed after the two the model declares.
static TAG_DISCRIMINATOR: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.ram#Tag$__type",
        NAMESPACE,
        DISCRIMINATOR_MEMBER,
    ),
    ShapeType::String,
    DISCRIMINATOR_MEMBER,
    2,
);

/// `Tag$key`, as the attribute emits it for the modelled member.
static TAG_KEY: Schema = Schema::new_member(
    ShapeId::from_static("com.amazonaws.ram#Tag$key", NAMESPACE, "key"),
    ShapeType::String,
    "key",
    0,
);

/// `Tag$value`, as the attribute emits it for the modelled member.
static TAG_VALUE: Schema = Schema::new_member(
    ShapeId::from_static("com.amazonaws.ram#Tag$value", NAMESPACE, "value"),
    ShapeType::String,
    "value",
    1,
);

/// One stored tag pair, written as a self-describing document: the structure
/// discriminator naming the `Tag` shape, then the pair's own members.
///
/// Borrows the `Tag` the handler already built, so the two mutation handlers keep
/// reporting the pairs they read back out of the store and nothing copies them.
#[derive(Debug)]
pub struct DocumentTag<'a>(pub &'a Tag);

impl SerializableStruct for DocumentTag<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&TAG_DISCRIMINATOR, tag_structure_name())?;
        if let Some(key) = self.0.key.as_ref() {
            s.write_string(&TAG_KEY, key)?;
        }
        if let Some(value) = self.0.value.as_ref() {
            s.write_string(&TAG_VALUE, value)?;
        }
        Ok(())
    }
}

/// Write `list` as the value of the `TagList` member `member`, each element a
/// [`DocumentTag`].
fn write_document_tag_list(
    s: &mut dyn ShapeSerializer,
    member: &Schema,
    list: &[Tag],
) -> Result<(), SerdeError> {
    s.write_list(member, &|element_serializer| {
        for tag in list {
            element_serializer.write_struct(&Tag::SCHEMA, &DocumentTag(tag))?;
        }
        Ok(())
    })
}

/// The schema and the serializer for a tag-mutation response whose single member is a
/// `TagList` reported as documents.
///
/// Stated once for the two shapes that need it, so the two cannot disagree about the
/// wire form of an entry.
macro_rules! document_tag_list_response {
    ($ty:ident, $schema_static:ident, $shape:literal, $local:literal, $field:ident, $wire:literal) => {
        #[allow(non_upper_case_globals)]
        impl $ty {
            const M_TAG_LIST: Schema = Schema::new_member(
                ShapeId::from_static(concat!($shape, "$", $wire), NAMESPACE, $wire),
                ShapeType::List,
                $wire,
                0,
            );

            pub const SCHEMA: Schema = Schema::new_struct(
                ShapeId::from_static($shape, NAMESPACE, $local),
                ShapeType::Structure,
                &[&Self::M_TAG_LIST],
            );
        }

        pub static $schema_static: &Schema = &$ty::SCHEMA;

        impl SerializableStruct for $ty {
            fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
                if let Some(list) = self.$field.as_ref() {
                    write_document_tag_list(s, &Self::M_TAG_LIST, list)?;
                }
                Ok(())
            }
        }
    };
}

document_tag_list_response!(
    TagResourceOutput,
    TAG_RESOURCE_OUTPUT_SCHEMA,
    "com.amazonaws.ram#TagResourceResponse",
    "TagResourceResponse",
    tags_created,
    "tagsCreated"
);

document_tag_list_response!(
    UntagResourceOutput,
    UNTAG_RESOURCE_OUTPUT_SCHEMA,
    "com.amazonaws.ram#UntagResourceResponse",
    "UntagResourceResponse",
    deleted_tags,
    "deletedTags"
);
