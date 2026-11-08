// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Codegen helpers for the `SmithyStruct` / `SmithyUnion` derive macros.
//!
//! Given a Smithy member shape and a Rust field type, emits the right
//! `read_*` / `write_*` calls. Recursive on lists, maps, structures, and
//! unions: a `list<list<string>>` member produces a nested
//! `read_list(... |d| read_list(... |d| read_string(...)))` block.
//!
//! The classifier deliberately ignores the Rust type beyond its shape
//! (`String`, `Vec<...>`, `HashMap<...>`, `Option<...>`, custom path).
//! All real type information comes from the Smithy model. The Rust type
//! determines:
//!
//! - whether the field is wrapped in `Option` (must match Smithy's
//!   `@required` / non-required for sanity);
//! - what concrete container to use (`Vec`, `HashMap`, `BTreeMap`, etc.);
//! - what the user-visible field type is for the `Some(...)` wrapper on
//!   read.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use serde_json::Value;
use std::collections::HashMap;
use syn::Type;

/// What kind of Smithy shape backs a member's target.
///
/// We match on this to decide which `read_*` / `write_*` to emit.
#[derive(Debug, Clone)]
pub enum ShapeKind<'a> {
    Boolean,
    Byte,
    Short,
    Integer,
    Long,
    Float,
    Double,
    BigInteger,
    BigDecimal,
    String,
    Blob,
    Timestamp,
    Document,
    /// Smithy `enum` shape — wire-encoded as a string.
    Enum,
    List {
        element: Box<ShapeKind<'a>>,
    },
    Map {
        value: Box<ShapeKind<'a>>,
    },
    /// Smithy `structure` shape. We don't emit anything inline — we
    /// delegate to the corresponding Rust type's `DeserializableStruct`
    /// impl.
    Structure,
    /// Smithy `union` shape. Same as structure: delegate.
    Union,
    /// Anything else (placeholder for unsupported types).
    Other(&'a str),
}

/// Walk a Smithy member's `target` shape and classify its kind.
///
/// `model` is the full `shapes` map from the Smithy model.
pub fn classify_member<'a>(
    member: &'a Value,
    model: &'a HashMap<String, Value>,
) -> Option<ShapeKind<'a>> {
    let target = member.get("target")?.as_str()?;
    classify_shape(target, model)
}

fn classify_shape<'a>(
    shape_id: &'a str,
    model: &'a HashMap<String, Value>,
) -> Option<ShapeKind<'a>> {
    // smithy.api prelude shapes
    match shape_id {
        "smithy.api#Boolean" | "smithy.api#PrimitiveBoolean" => return Some(ShapeKind::Boolean),
        "smithy.api#Byte" | "smithy.api#PrimitiveByte" => return Some(ShapeKind::Byte),
        "smithy.api#Short" | "smithy.api#PrimitiveShort" => return Some(ShapeKind::Short),
        "smithy.api#Integer" | "smithy.api#PrimitiveInteger" => return Some(ShapeKind::Integer),
        "smithy.api#Long" | "smithy.api#PrimitiveLong" => return Some(ShapeKind::Long),
        "smithy.api#Float" | "smithy.api#PrimitiveFloat" => return Some(ShapeKind::Float),
        "smithy.api#Double" | "smithy.api#PrimitiveDouble" => return Some(ShapeKind::Double),
        "smithy.api#BigInteger" => return Some(ShapeKind::BigInteger),
        "smithy.api#BigDecimal" => return Some(ShapeKind::BigDecimal),
        "smithy.api#String" => return Some(ShapeKind::String),
        "smithy.api#Blob" => return Some(ShapeKind::Blob),
        "smithy.api#Timestamp" => return Some(ShapeKind::Timestamp),
        "smithy.api#Document" => return Some(ShapeKind::Document),
        _ => {}
    }

    let shape = model.get(shape_id)?;
    let shape_type = shape.get("type")?.as_str()?;
    Some(match shape_type {
        "boolean" => ShapeKind::Boolean,
        "byte" => ShapeKind::Byte,
        "short" => ShapeKind::Short,
        "integer" => ShapeKind::Integer,
        "long" => ShapeKind::Long,
        "float" => ShapeKind::Float,
        "double" => ShapeKind::Double,
        "bigInteger" => ShapeKind::BigInteger,
        "bigDecimal" => ShapeKind::BigDecimal,
        "string" => ShapeKind::String,
        "blob" => ShapeKind::Blob,
        "timestamp" => ShapeKind::Timestamp,
        "document" => ShapeKind::Document,
        "enum" => ShapeKind::Enum,
        "list" => {
            let elem_target = shape
                .get("member")
                .and_then(|m| m.get("target"))
                .and_then(|t| t.as_str())?;
            ShapeKind::List {
                element: Box::new(classify_shape(elem_target, model)?),
            }
        }
        "map" => {
            // Map keys are always strings in Smithy; we only need the value kind.
            let value_target = shape
                .get("value")
                .and_then(|m| m.get("target"))
                .and_then(|t| t.as_str())?;
            ShapeKind::Map {
                value: Box::new(classify_shape(value_target, model)?),
            }
        }
        "structure" => ShapeKind::Structure,
        "union" => ShapeKind::Union,
        "intEnum" => ShapeKind::Integer,
        other => ShapeKind::Other(other),
    })
}

/// True if `shape_id` names a `union` shape carrying `@streaming` — Smithy's
/// event-stream signal. Such a union is serialized as a `vnd.amazon.eventstream`
/// body, not a plain awsJson object.
pub fn is_event_stream_union(shape_id: &str, model: &HashMap<String, Value>) -> bool {
    model
        .get(shape_id)
        .map(|shape| {
            shape.get("type").and_then(|t| t.as_str()) == Some("union")
                && shape
                    .get("traits")
                    .and_then(|t| t.as_object())
                    .is_some_and(|t| t.contains_key("smithy.api#streaming"))
        })
        .unwrap_or(false)
}

// =============================================================================
// Code emission
// =============================================================================

/// Whether a value of this shape can report a member absent from a structure
/// nested under it — i.e. whether there is any position worth naming.
///
/// A scalar element never can, so a collection of scalars emits exactly the code
/// it emitted before positions were tracked. Recursion terminates on the finite
/// shape tree `classify_shape` built.
fn can_carry_missing_member(kind: &ShapeKind<'_>) -> bool {
    match kind {
        ShapeKind::Structure | ShapeKind::Union => true,
        ShapeKind::List { element } => can_carry_missing_member(element),
        ShapeKind::Map { value } => can_carry_missing_member(value),
        _ => false,
    }
}

/// Emit a snippet that *reads* a value of `kind` from a deserializer named
/// `d` against a member-schema reference `member_expr`. Produces an
/// expression of type `T` (for whatever Rust type the user declared).
///
/// `target_ty` is the Rust type expected after the read. For lists/maps we
/// peek into it to discover the inner Rust type (e.g., `Vec<String>` ->
/// `String`).
pub fn emit_read(
    kind: &ShapeKind<'_>,
    target_ty: &Type,
    member_expr: &TokenStream,
    d: &TokenStream,
) -> syn::Result<TokenStream> {
    Ok(match kind {
        ShapeKind::Boolean => quote! { #d.read_boolean(#member_expr)? },
        ShapeKind::Byte => quote! { #d.read_byte(#member_expr)? },
        ShapeKind::Short => quote! { #d.read_short(#member_expr)? },
        ShapeKind::Integer => quote! { #d.read_integer(#member_expr)? },
        ShapeKind::Long => quote! { #d.read_long(#member_expr)? },
        ShapeKind::Float => quote! { #d.read_float(#member_expr)? },
        ShapeKind::Double => quote! { #d.read_double(#member_expr)? },
        ShapeKind::BigInteger => quote! { #d.read_big_integer(#member_expr)? },
        ShapeKind::BigDecimal => quote! { #d.read_big_decimal(#member_expr)? },
        ShapeKind::String | ShapeKind::Enum => {
            // Enums read as strings; user type may be a Rust enum that
            // implements `From<&str>` or `FromStr`. We emit
            // `read_string(...)?` and let the caller convert if needed.
            // For simplicity we'll detect Rust enums later via a
            // FromSmithyString trait (see wire.rs).
            //
            // For the common case where the Rust type is `String`, the
            // expression is just the string. For other types we wrap with
            // FromSmithyString.
            if is_string_type(target_ty) {
                quote! { #d.read_string(#member_expr)? }
            } else {
                // Use the FromSmithyString trait from `crate::wire`
                quote! {
                    {
                        let __s = #d.read_string(#member_expr)?;
                        <#target_ty as ::smithy_server_runtime::wire::FromSmithyString>::from_smithy_string(&__s)
                            .map_err(|e| ::aws_smithy_schema::serde::SerdeError::custom(format!("{}", e)))?
                    }
                }
            }
        }
        ShapeKind::Blob => quote! {
            <#target_ty as ::std::convert::From<::std::vec::Vec<u8>>>::from(
                #d.read_blob(#member_expr)?.into_inner()
            )
        },
        ShapeKind::Timestamp => quote! { #d.read_timestamp(#member_expr)? },
        ShapeKind::Document => quote! { #d.read_document(#member_expr)? },
        ShapeKind::List { element } => {
            // Inner Rust type for the Vec.
            let inner_ty = vec_inner(target_ty).ok_or_else(|| {
                syn::Error::new_spanned(
                    target_ty,
                    "list-typed Smithy member requires a Vec<T> Rust field",
                )
            })?;
            // Element member schema: pre-emit a static `M_ELEM` reference.
            // Here we just use `#member_expr` as the schema — the
            // deserializer doesn't actually need a member schema for the
            // element when the codec is JSON.
            let inner_read = emit_read(element, inner_ty, member_expr, &quote! { __dl })?;
            if can_carry_missing_member(element) {
                // Count the element being read so an absent required member under
                // it reports its position. The counter is read AFTER the walk: a
                // failing element's `?` leaves the consumer and `read_list`
                // propagates it unchanged, so `__pos` still names that element.
                // Every `read_list` in this crate calls the consumer exactly once
                // per element, in order — that contract is what makes this correct.
                quote! {
                    {
                        let mut __out: Vec<#inner_ty> = Vec::new();
                        let mut __pos = 0usize;
                        #d.read_list(#member_expr, &mut |__dl| {
                            __pos += 1;
                            __out.push(#inner_read);
                            Ok(())
                        })
                        .map_err(|__e| {
                            ::aws_smithy_schema_server::within_list_element(__e, __pos)
                        })?;
                        __out
                    }
                }
            } else {
                // A scalar element cannot hold a required member, so there is no
                // position to report and nothing to count.
                quote! {
                    {
                        let mut __out: Vec<#inner_ty> = Vec::new();
                        #d.read_list(#member_expr, &mut |__dl| {
                            __out.push(#inner_read);
                            Ok(())
                        })?;
                        __out
                    }
                }
            }
        }
        ShapeKind::Map { value } => {
            let (key_ty, value_ty) = map_inner(target_ty).ok_or_else(|| {
                syn::Error::new_spanned(
                    target_ty,
                    "map-typed Smithy member requires a HashMap<String, V> or similar",
                )
            })?;
            // Most Smithy maps are keyed by a string — we accept any
            // FromStr-compatible key type but will mostly see String here.
            let key_assign = if is_string_type(&key_ty) {
                quote! { __k }
            } else {
                quote! { <#key_ty as ::std::str::FromStr>::from_str(&__k)
                .map_err(|e| ::aws_smithy_schema::serde::SerdeError::custom(format!("{}", e)))? }
            };
            let value_read = emit_read(value, &value_ty, member_expr, &quote! { __dm })?;
            // Construct via `<MapTy as Default>::default()` so the same
            // codegen works for HashMap, BTreeMap, IndexMap, Item, etc.
            if can_carry_missing_member(value) {
                // Name the entry by its key. The value is read BEFORE the key is
                // converted so the error can borrow `__k` rather than clone it.
                quote! {
                    {
                        let mut __out: #target_ty = ::std::default::Default::default();
                        #d.read_map(#member_expr, &mut |__k, __dm| {
                            let __value = (|| -> ::std::result::Result<
                                #value_ty,
                                ::aws_smithy_schema::serde::SerdeError,
                            > {
                                // `#value_read` ends in `?`; bind before wrapping
                                // in `Ok`, or clippy flags needless_question_mark.
                                let __read = #value_read;
                                ::std::result::Result::Ok(__read)
                            })()
                            .map_err(|__e| {
                                ::aws_smithy_schema_server::within_map_entry(__e, &__k)
                            })?;
                            __out.insert(#key_assign, __value);
                            Ok(())
                        })?;
                        __out
                    }
                }
            } else {
                quote! {
                    {
                        let mut __out: #target_ty = ::std::default::Default::default();
                        #d.read_map(#member_expr, &mut |__k, __dm| {
                            let __key = #key_assign;
                            let __value = #value_read;
                            __out.insert(__key, __value);
                            Ok(())
                        })?;
                        __out
                    }
                }
            }
        }
        ShapeKind::Structure | ShapeKind::Union => match unwrap_box(target_ty) {
            // A `Box<T>` field is how a self-referential shape is given finite
            // size. `Box<T>` is not itself a `DeserializableStruct`, so read the
            // BOXED type and re-box the value.
            Some(inner) => quote! {
                ::std::boxed::Box::new(
                    <#inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(#d)?
                )
            },
            None => quote! {
                <#target_ty as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(#d)?
            },
        },
        ShapeKind::Other(other) => {
            return Err(syn::Error::new_spanned(
                target_ty,
                format!("unsupported Smithy shape type: {other}"),
            ))
        }
    })
}

/// Emit a snippet that *writes* a value of `kind` to a serializer named
/// `s` against a member-schema reference `member_expr`.
/// `value_expr` is a Rust expression yielding the value to write.
pub fn emit_write(
    kind: &ShapeKind<'_>,
    value_ty: &Type,
    member_expr: &TokenStream,
    value_expr: &TokenStream,
    s: &TokenStream,
) -> syn::Result<TokenStream> {
    Ok(match kind {
        ShapeKind::Boolean => quote! { #s.write_boolean(#member_expr, *#value_expr)?; },
        ShapeKind::Byte => quote! { #s.write_byte(#member_expr, *#value_expr)?; },
        ShapeKind::Short => quote! { #s.write_short(#member_expr, *#value_expr)?; },
        ShapeKind::Integer => quote! { #s.write_integer(#member_expr, *#value_expr)?; },
        ShapeKind::Long => quote! { #s.write_long(#member_expr, *#value_expr)?; },
        ShapeKind::Float => quote! { #s.write_float(#member_expr, *#value_expr)?; },
        ShapeKind::Double => quote! { #s.write_double(#member_expr, *#value_expr)?; },
        ShapeKind::BigInteger => quote! { #s.write_big_integer(#member_expr, #value_expr)?; },
        ShapeKind::BigDecimal => quote! { #s.write_big_decimal(#member_expr, #value_expr)?; },
        ShapeKind::String | ShapeKind::Enum => {
            // Both branches call an inherent `as_str(&self) -> &str` —
            // generated by `smithy(enum_for = ...)` for Smithy enums, and
            // already present on `String`.
            quote! { #s.write_string(#member_expr, #value_expr.as_str())?; }
        }
        ShapeKind::Blob => quote! {
            #s.write_blob(
                #member_expr,
                <#value_ty as ::std::convert::AsRef<[u8]>>::as_ref(#value_expr),
            )?;
        },
        ShapeKind::Timestamp => quote! { #s.write_timestamp(#member_expr, #value_expr)?; },
        ShapeKind::Document => quote! { #s.write_document(#member_expr, #value_expr)?; },
        ShapeKind::List { element } => {
            let inner_ty = vec_inner(value_ty).ok_or_else(|| {
                syn::Error::new_spanned(
                    value_ty,
                    "list-typed Smithy member requires a Vec<T> Rust field",
                )
            })?;
            // For struct/union items in a list, use the struct's own SCHEMA
            // so the XML element name is correct (e.g., "ResourceRecordSet" not "ResourceRecordSets")
            let item_schema_expr = match element.as_ref() {
                ShapeKind::Structure | ShapeKind::Union => quote! { &#inner_ty::SCHEMA },
                _ => member_expr.clone(),
            };
            let inner_write = emit_write(
                element,
                inner_ty,
                &item_schema_expr,
                &quote! { __it },
                &quote! { __sl },
            )?;
            quote! {
                #s.write_list(#member_expr, &|__sl| {
                    for __it in #value_expr.iter() {
                        #inner_write
                    }
                    Ok(())
                })?;
            }
        }
        ShapeKind::Map { value } => {
            let (_key_ty, value_ty_inner) = map_inner(value_ty).ok_or_else(|| {
                syn::Error::new_spanned(
                    value_ty,
                    "map-typed Smithy member requires a HashMap<String, V> or similar",
                )
            })?;
            let value_write = emit_write(
                value,
                &value_ty_inner,
                member_expr,
                &quote! { __v },
                &quote! { __sm },
            )?;
            quote! {
                #s.write_map(#member_expr, &|__sm| {
                    for (__k, __v) in #value_expr.iter() {
                        __sm.write_string(#member_expr, __k.as_ref())?;
                        #value_write
                    }
                    Ok(())
                })?;
            }
        }
        ShapeKind::Structure | ShapeKind::Union => {
            quote! { #s.write_struct(#member_expr, #value_expr)?; }
        }
        ShapeKind::Other(other) => {
            return Err(syn::Error::new_spanned(
                value_ty,
                format!("unsupported Smithy shape type: {other}"),
            ))
        }
    })
}

// =============================================================================
// Schema constant emission
// =============================================================================

/// Map a Smithy shape kind to its `aws_smithy_schema::ShapeType` variant
/// for use in `Schema::new_member` declarations.
pub fn shape_type_ident(kind: &ShapeKind<'_>) -> TokenStream {
    let ident = match kind {
        ShapeKind::Boolean => "Boolean",
        ShapeKind::Byte => "Byte",
        ShapeKind::Short => "Short",
        ShapeKind::Integer => "Integer",
        ShapeKind::Long => "Long",
        ShapeKind::Float => "Float",
        ShapeKind::Double => "Double",
        ShapeKind::BigInteger => "BigInteger",
        ShapeKind::BigDecimal => "BigDecimal",
        ShapeKind::String | ShapeKind::Enum => "String",
        ShapeKind::Blob => "Blob",
        ShapeKind::Timestamp => "Timestamp",
        ShapeKind::Document => "Document",
        ShapeKind::List { .. } => "List",
        ShapeKind::Map { .. } => "Map",
        ShapeKind::Structure => "Structure",
        ShapeKind::Union => "Union",
        ShapeKind::Other(_) => "Boolean", // fallback; should never be reached
    };
    let id = format_ident!("{}", ident);
    quote! { ::aws_smithy_schema::ShapeType::#id }
}

// =============================================================================
// Type-shape helpers
// =============================================================================

/// True if `ty` is the Rust `String` type (or `&str`).
pub fn is_string_type(ty: &Type) -> bool {
    if let Type::Path(p) = ty {
        if let Some(seg) = p.path.segments.last() {
            return seg.ident == "String" || seg.ident == "str";
        }
    }
    false
}

/// Strip an outer `Option<...>` and return the inner type. Returns `None`
/// if the type isn't `Option<T>`.
pub fn unwrap_option(ty: &Type) -> Option<&Type> {
    if let Type::Path(p) = ty {
        if let Some(seg) = p.path.segments.last() {
            if seg.ident == "Option" {
                if let PathArguments::AngleBracketed(args) = &seg.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return Some(inner);
                    }
                }
            }
        }
    }
    None
}

/// Strip an outer `Box<...>` and return the boxed type.
pub fn unwrap_box(ty: &Type) -> Option<&Type> {
    if let Type::Path(p) = ty {
        if let Some(seg) = p.path.segments.last() {
            if seg.ident == "Box" {
                if let PathArguments::AngleBracketed(args) = &seg.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return Some(inner);
                    }
                }
            }
        }
    }
    None
}

/// Strip an outer `Vec<...>` and return the inner type.
pub fn vec_inner(ty: &Type) -> Option<&Type> {
    if let Type::Path(p) = ty {
        if let Some(seg) = p.path.segments.last() {
            if seg.ident == "Vec" {
                if let PathArguments::AngleBracketed(args) = &seg.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return Some(inner);
                    }
                }
            }
        }
    }
    None
}

/// Strip an outer `HashMap<K, V>` (or `BTreeMap`, or any 2-arg map type
/// alias like `Item`) and return `(K, V)`.
pub fn map_inner(ty: &Type) -> Option<(Type, Type)> {
    if let Type::Path(p) = ty {
        if let Some(seg) = p.path.segments.last() {
            // Accept any path whose last segment has 2 generic args.
            if let PathArguments::AngleBracketed(args) = &seg.arguments {
                if args.args.len() == 2 {
                    let mut iter = args.args.iter();
                    if let (
                        Some(syn::GenericArgument::Type(k)),
                        Some(syn::GenericArgument::Type(v)),
                    ) = (iter.next(), iter.next())
                    {
                        return Some((k.clone(), v.clone()));
                    }
                }
            }
            // Type aliases like `Item` (which is HashMap<String,
            // AttributeValue>) — we can't resolve them here without a
            // full type-checker. Fall back to `(String, AttributeValue)`
            // for the well-known alias.
            if seg.ident == "Item" {
                let k: Type = syn::parse_str("String").unwrap();
                let v: Type = syn::parse_str("crate::operations::AttributeValue").unwrap();
                return Some((k, v));
            }
        }
    }
    None
}

use syn::PathArguments;

// =============================================================================
// Constraint extraction (trait-map capture)
// =============================================================================
//
// Codegen captures constraint traits as runtime-traversable data on the member
// schema's generic `TraitMap`; it bakes in **no** validation logic. The
// dispatch-boundary `ValidatingSerializer` walk (in `aws-smithy-schema-server`)
// reads these traits and enforces them. See `emit_constraint_trait_inserts`.

/// Constraints extracted from a Smithy shape's traits. Each field mirrors a
/// constraint trait the [`crate`]'s framework enforces at runtime.
#[derive(Debug, Default, Clone)]
pub struct Constraints {
    /// `@length` (min, max). Applies to string, blob, list, map.
    pub length: Option<(Option<i64>, Option<i64>)>,
    /// `@range` (min, max). Applies to numeric types.
    pub range: Option<(Option<f64>, Option<f64>)>,
    /// `@pattern` regex source. Applies to string.
    pub pattern: Option<String>,
    /// Older `@enum` string-constraint value set. Applies to plain `string`
    /// members (NOT typed `enum` shapes, which codegen handles separately).
    pub enum_values: Option<Vec<String>>,
    /// Whether `enum_values` match without regard to ASCII case. Set only for a header
    /// the service serves in the case its clients send (see
    /// [`binds_a_client_cased_header`]); every other value set is matched exactly.
    pub enum_ignores_case: bool,
    /// `@uniqueItems`. Applies to list.
    pub unique_items: bool,
    /// `@sensitive`: a violation never shows the value. Applies to every kind.
    pub sensitive: bool,
}

impl Constraints {
    /// True when no constraint trait was captured.
    pub fn is_empty(&self) -> bool {
        self.length.is_none()
            && self.range.is_none()
            && self.pattern.is_none()
            && self.enum_values.is_none()
            && !self.unique_items
            && !self.sensitive
    }
}

/// Extract every supported constraint trait for a member: the target shape's,
/// plus any the MEMBER declares itself.
///
/// A constraint trait may be written on either, and the two mean different
/// things: on a shape it constrains every member targeting that shape, on a
/// member it constrains only that member. The selectors for `@length`, `@range`,
/// `@pattern` and `@uniqueItems` all admit a member target, so reading only the
/// shape silently drops every bound written the second way.
///
/// **A member trait is read only where the target shape declares no trait of the
/// same kind.** Where both declare one they can disagree — a member narrowing
/// the minimum while the shape also sets a maximum, say — and the model does not
/// record which the service enforces. Preferring the member there would stop
/// enforcing a bound this walk enforces today, which is the one direction that
/// can newly ACCEPT a request the emulator currently rejects. So the shape keeps
/// precedence and the conflicting case is left to a change that can show what
/// the service accepts; the non-conflicting case carries no such question,
/// because nothing enforces those bounds at all until this reads them.
pub fn extract_constraints(smithy_member: &Value, model: &HashMap<String, Value>) -> Constraints {
    let target = smithy_member.get("target").and_then(|t| t.as_str());
    let mut c = match target {
        Some(target) => extract_constraints_for_shape(target, model),
        None => Constraints::default(),
    };
    if let Some(member_traits) = smithy_member.get("traits").and_then(|t| t.as_object()) {
        // `@enum` is read only for a `string` shape, so the member's own traits are
        // read against the TARGET's type, not the member's (a member has no `type`).
        let shape_type = target
            .and_then(|t| model.get(t))
            .and_then(|s| s.get("type"))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        merge_constraint_traits(&mut c, member_traits, shape_type);
    }
    if c.enum_values.is_some() && binds_a_negotiated_header(smithy_member) {
        c.enum_values = None;
    } else if c.enum_values.is_some() && binds_a_client_cased_header(smithy_member) {
        c.enum_ignores_case = true;
    }
    c
}

/// Whether the member is bound to an HTTP header whose value AWS's own clients send in a
/// case the model's value set does not list, and which the service accepts in that case.
///
/// `x-amz-checksum-mode` is modeled as the enum `[ENABLED]`, but the AWS Common Runtime
/// (CRT) S3 client, which the AWS CLI v2 can pick for `aws s3 cp`, `sync` and `mv`, sends
/// `x-amz-checksum-mode: enabled`, and S3 serves those requests. Enforcing the set as
/// case-sensitive equality refuses every such download: HeadObject answers a bare 400 and
/// GetObject a `ValidationException`.
///
/// `x-amz-sdk-checksum-algorithm` is modeled as the checksum algorithm set
/// (`CRC32`, `CRC32C`, ...), but the AWS SDK for Swift sends `crc32` in lower case when
/// the caller chose no algorithm, and S3 serves it: `crc32`, `CRC32` and `Crc32` all
/// pass there, while a value outside the set is still refused. Enforcing the set as
/// case-sensitive equality refuses every default Swift `PutObject` and `UploadPart`.
///
/// The set is still enforced, but matched without regard to ASCII case: `ENABLED`,
/// `enabled` and `Enabled` pass, and any other value is refused as before. Every other
/// bound on the member still applies.
fn binds_a_client_cased_header(smithy_member: &Value) -> bool {
    const CLIENT_CASED: [&str; 2] = ["x-amz-checksum-mode", "x-amz-sdk-checksum-algorithm"];
    smithy_member
        .get("traits")
        .and_then(|t| t.get("smithy.api#httpHeader"))
        .and_then(|h| h.as_str())
        .is_some_and(|name| CLIENT_CASED.contains(&name.to_ascii_lowercase().as_str()))
}

/// Whether the member is bound to an HTTP header whose own grammar is wider than any
/// closed value set.
///
/// `Accept`, `Accept-Encoding`, `Accept-Language` and `Content-Type` are defined by
/// HTTP itself to admit a comma-separated list, quality values and `*` — so
/// `Accept-Encoding: gzip, deflate` is legal and does request gzip, and `Accept: */*`
/// is what a client sends by default. A model that targets such a member at an `enum`
/// is naming the values the SERVICE acts on, not restricting the header's syntax, so
/// enforcing the set as whole-value equality refuses requests no HTTP front end could
/// refuse.
///
/// Only enum membership is suppressed. `@length`, `@pattern` and `@range` on such a
/// member still apply: each bounds a value the model really does constrain, and none
/// of them asserts the value is drawn from a closed set.
fn binds_a_negotiated_header(smithy_member: &Value) -> bool {
    const NEGOTIATED: [&str; 4] = [
        "accept",
        "accept-encoding",
        "accept-language",
        "content-type",
    ];
    smithy_member
        .get("traits")
        .and_then(|t| t.get("smithy.api#httpHeader"))
        .and_then(|h| h.as_str())
        .is_some_and(|name| NEGOTIATED.contains(&name.to_ascii_lowercase().as_str()))
}

/// The scalar leaf of a collection member, and that leaf shape's own
/// constraints, for the validating walk's `ElementConstraintsTrait`.
///
/// Scalar list elements and map values share the collection member's schema and
/// so cannot carry their own traits directly. This captures them, descending
/// every collection level — `list<list<ConfigArn>>` resolves to `ConfigArn` —
/// because the walk reads the wrapper only on a scalar leaf.
///
/// The kind tree and the model are two spellings of one shape, so both are
/// descended together and the returned kind always matches the returned
/// constraints. Descending only one, or only one level, silently mismatches
/// them: the intermediate collection's `@length` is an element *count*, and
/// applied to a leaf it demands that every `ConfigArn` be two characters long.
///
/// `None` when the member is not a collection or when its leaf shape carries no
/// constraints. Which of those constraints actually apply to the leaf is
/// [`emit_constraint_trait_inserts`]'s decision, made from the returned kind —
/// so a structure or union leaf contributes nothing, and neither does a trait
/// that does not fit the leaf's kind. Map *keys* are not captured.
pub fn extract_element_constraints<'a, 'k>(
    kind: &'a ShapeKind<'k>,
    smithy_member: &Value,
    model: &HashMap<String, Value>,
) -> Option<(&'a ShapeKind<'k>, Constraints)> {
    let mut kind = collection_element_kind(kind)?;
    let mut target =
        collection_element_target(smithy_member.get("target").and_then(|t| t.as_str())?, model)?;
    // The kind drives the descent and the model follows it, so the loop can only
    // end on a non-collection kind. That is what keeps an intermediate
    // collection's element-count `@length` out of the returned constraints: were
    // the model to lead, a shape it could not resolve would leave `kind` on a
    // collection and hand that count back as if it bounded a leaf.
    while let Some(next_kind) = collection_element_kind(kind) {
        target = collection_element_target(target, model)?;
        kind = next_kind;
    }
    let c = extract_constraints_for_shape(target, model);
    (!c.is_empty()).then_some((kind, c))
}

/// The element kind (list) or value kind (map) one level in, if `kind` is a
/// collection.
fn collection_element_kind<'a, 'k>(kind: &'a ShapeKind<'k>) -> Option<&'a ShapeKind<'k>> {
    match kind {
        ShapeKind::List { element } => Some(element),
        ShapeKind::Map { value } => Some(value),
        _ => None,
    }
}

/// The shape id of `target`'s element (list) or value (map), if `target` names a
/// collection shape.
fn collection_element_target<'m>(
    target: &str,
    model: &'m HashMap<String, Value>,
) -> Option<&'m str> {
    let shape = model.get(target)?;
    match shape.get("type")?.as_str()? {
        "list" => shape.get("member")?.get("target")?.as_str(),
        "map" => shape.get("value")?.get("target")?.as_str(),
        _ => None,
    }
}

/// Extract every supported constraint trait from a shape by its id.
fn extract_constraints_for_shape(target: &str, model: &HashMap<String, Value>) -> Constraints {
    let mut c = Constraints::default();
    let shape = match model.get(target) {
        Some(s) => s,
        None => return c,
    };
    let shape_type = shape.get("type").and_then(|t| t.as_str()).unwrap_or("");
    // An `enum` shape carries its membership in its MEMBERS, not in a shape-level
    // trait, and most declare no shape-level traits at all — so this must run
    // before the `traits` bail-out below or it is unreachable for them.
    if shape_type == "enum" {
        c.enum_values = enum_shape_values(shape);
    }
    if let Some(traits) = shape.get("traits").and_then(|t| t.as_object()) {
        merge_constraint_traits(&mut c, traits, shape_type);
    }
    c
}

/// The wire values of an `enum` shape, read from its members.
///
/// Each member's `@enumValue` is the string that goes on the wire; it is often
/// not the member name (a member `EC_PRIME256_V1` may encode as
/// `EC_prime256v1`), so the name is only a fallback for a member that declares
/// no value. Returns `None` for a shape with no members, since an empty set
/// would reject every value.
fn enum_shape_values(shape: &Value) -> Option<Vec<String>> {
    let members = shape.get("members").and_then(|m| m.as_object())?;
    let values: Vec<String> = members
        .iter()
        .map(|(name, member)| {
            member
                .get("traits")
                .and_then(|t| t.get("smithy.api#enumValue"))
                .and_then(|v| v.as_str())
                .unwrap_or(name)
                .to_string()
        })
        .collect();
    (!values.is_empty()).then_some(values)
}

/// Read every supported constraint trait out of one `traits` map into `c`,
/// leaving any kind `c` already carries untouched.
///
/// Shared by the shape and member readers so the two cannot drift: a trait kind
/// added here is captured from both positions, and the "first writer wins" rule
/// is what makes the target shape take precedence over the member (see
/// [`extract_constraints`]). `shape_type` is the TARGET shape's type — `@enum`
/// applies only to a plain `string`, and a member carries no type of its own.
fn merge_constraint_traits(
    c: &mut Constraints,
    traits: &serde_json::Map<String, Value>,
    shape_type: &str,
) {
    if let Some(length) = traits
        .get("smithy.api#length")
        .filter(|_| c.length.is_none())
    {
        let min = length.get("min").and_then(|v| v.as_i64());
        let max = length.get("max").and_then(|v| v.as_i64());
        c.length = Some((min, max));
    }
    if let Some(range) = traits.get("smithy.api#range").filter(|_| c.range.is_none()) {
        let min = range.get("min").and_then(|v| v.as_f64());
        let max = range.get("max").and_then(|v| v.as_f64());
        c.range = Some((min, max));
    }
    if let Some(pattern) = traits
        .get("smithy.api#pattern")
        .filter(|_| c.pattern.is_none())
        .and_then(|v| v.as_str())
    {
        c.pattern = Some(pattern.to_string());
    }
    // The older `smithy.api#enum` string-constraint trait applies only to a plain
    // `string` shape; an `enum` shape's membership comes from its members instead
    // and is already captured by the caller. Both spellings land in the same
    // `enum_values`, because both encode as a string on the wire.
    if shape_type == "string" && c.enum_values.is_none() {
        if let Some(values) = traits.get("smithy.api#enum").and_then(|v| v.as_array()) {
            let vals: Vec<String> = values
                .iter()
                .filter_map(|entry| {
                    // Smithy 1.0 `@enum` is a list of objects `{ "value": .. }`;
                    // the value (or, legacy, `name`) is the wire string.
                    entry
                        .get("value")
                        .or_else(|| entry.get("name"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
                .collect();
            if !vals.is_empty() {
                c.enum_values = Some(vals);
            }
        }
    }
    // `@uniqueItems` is a marker, so there is no value to disagree about and no
    // precedence question: either position setting it means the same thing.
    if traits.contains_key("smithy.api#uniqueItems") {
        c.unique_items = true;
    }
    // `@sensitive` is a marker too, on the member or its target.
    if traits.contains_key("smithy.api#sensitive") {
        c.sensitive = true;
    }
}

/// Emit the `TraitMap` insert statements for every constraint captured from a
/// member, applicable to the member's shape `kind`. Returns the list of
/// `m.insert(Box::new(<Trait>))` statements (empty when nothing applies); the
/// caller wraps these in a `LazyLock<TraitMap>` static.
///
/// This is the *only* constraint codegen: it captures trait data. No `if len <
/// N` checks are emitted anywhere — the framework walk performs all enforcement.
pub fn emit_constraint_trait_inserts(
    constraints: &Constraints,
    kind: &ShapeKind<'_>,
) -> Vec<TokenStream> {
    let mut inserts = Vec::new();

    // `@length`: string / blob / list / map.
    if let Some((min, max)) = constraints.length {
        if matches!(
            kind,
            ShapeKind::String
                | ShapeKind::Enum
                | ShapeKind::Blob
                | ShapeKind::List { .. }
                | ShapeKind::Map { .. }
        ) {
            let min_tok = opt_i64(min);
            let max_tok = opt_i64(max);
            inserts.push(quote! {
                m.insert(::std::boxed::Box::new(
                    ::aws_smithy_schema_server::constraints::LengthTrait::new(#min_tok, #max_tok)
                ));
            });
        }
    }

    // `@range`: numeric types.
    if let Some((min, max)) = constraints.range {
        if matches!(
            kind,
            ShapeKind::Byte
                | ShapeKind::Short
                | ShapeKind::Integer
                | ShapeKind::Long
                | ShapeKind::Float
                | ShapeKind::Double
        ) {
            let min_tok = opt_f64(min);
            let max_tok = opt_f64(max);
            inserts.push(quote! {
                m.insert(::std::boxed::Box::new(
                    ::aws_smithy_schema_server::constraints::RangeTrait::new(#min_tok, #max_tok)
                ));
            });
        }
    }

    // `@pattern`: string.
    if let Some(pattern) = &constraints.pattern {
        if matches!(kind, ShapeKind::String) {
            inserts.push(quote! {
                m.insert(::std::boxed::Box::new(
                    ::aws_smithy_schema_server::constraints::PatternTrait::new(#pattern)
                ));
            });
        }
    }

    // Enum membership: a plain `string` carrying `@enum`, or an `enum` shape.
    // Both encode as a string, so one `EnumTrait` serves both — and an `enum`
    // shape is generated as a `String` field, so nothing else checks it.
    if let Some(values) = &constraints.enum_values {
        if matches!(kind, ShapeKind::String | ShapeKind::Enum) {
            let value_lits = values.iter().map(|v| quote! { #v });
            let ctor = if constraints.enum_ignores_case {
                quote! { new_ignoring_case }
            } else {
                quote! { new }
            };
            inserts.push(quote! {
                m.insert(::std::boxed::Box::new(
                    ::aws_smithy_schema_server::constraints::EnumTrait::#ctor(&[#(#value_lits),*])
                ));
            });
        }
    }

    // `@uniqueItems`: list.
    if constraints.unique_items && matches!(kind, ShapeKind::List { .. }) {
        inserts.push(quote! {
            m.insert(::std::boxed::Box::new(
                ::aws_smithy_schema_server::constraints::UniqueItemsTrait::new()
            ));
        });
    }

    // `@sensitive`: any kind, so the walk can hide the value of any violation.
    if constraints.sensitive {
        inserts.push(quote! {
            m.insert(::std::boxed::Box::new(::aws_smithy_schema::traits::SensitiveTrait));
        });
    }

    inserts
}

fn opt_i64(v: Option<i64>) -> TokenStream {
    match v {
        Some(v) => quote! { Some(#v) },
        None => quote! { None },
    }
}

fn opt_f64(v: Option<f64>) -> TokenStream {
    match v {
        Some(v) => quote! { Some(#v) },
        None => quote! { None },
    }
}

/// Auth scheme traits a service can apply, alphabetical: Smithy's order when no `@auth` lists them.
/// The models carry no `@authDefinition` shapes to discover them from, so a new scheme is added here.
const AUTH_SCHEME_TRAITS: [&str; 6] = [
    "aws.auth#sigv4",
    "aws.auth#sigv4a",
    "smithy.api#httpApiKeyAuth",
    "smithy.api#httpBasicAuth",
    "smithy.api#httpBearerAuth",
    "smithy.api#httpDigestAuth",
];

/// The operation's effective auth schemes: its `@auth`, else the service's `@auth`, else the
/// schemes the service applies; none for `@optionalAuth` or an empty `@auth`, which are served
/// unsigned. A service applying no known scheme gets SigV4, so an unlisted trait fails closed.
pub fn operation_auth_schemes(operation_shape: &Value, service_shape: &Value) -> Vec<String> {
    let traits = |shape: &Value| shape.get("traits").cloned().unwrap_or_default();
    let (operation, service) = (traits(operation_shape), traits(service_shape));
    if operation.get("smithy.api#optionalAuth").is_some() {
        return Vec::new();
    }
    let listed = operation
        .get("smithy.api#auth")
        .or_else(|| service.get("smithy.api#auth"));
    let applied = AUTH_SCHEME_TRAITS
        .iter()
        .filter(|t| service.get(**t).is_some());
    match listed.and_then(Value::as_array) {
        Some(schemes) => schemes
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        None => match applied.map(|t| t.to_string()).collect::<Vec<_>>() {
            found if found.is_empty() => vec![AUTH_SCHEME_TRAITS[0].to_string()],
            found => found,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ty(src: &str) -> Type {
        syn::parse_str(src).expect("type parses")
    }

    fn rendered(ty: &Type) -> String {
        quote! { #ty }.to_string()
    }

    // ---- element constraints for collection members -------------------------
    //
    // The fixture is `groundstation`'s real nesting, which exercises every level:
    //
    //     DataflowEdgeList = list<DataflowEdge>   @length(min: 0, max: 500)
    //     DataflowEdge     = list<ConfigArn>      @length(min: 2, max: 2)
    //     ConfigArn        = string               @length(min: 82, max: 424)
    //                                            @pattern("^arn:aws:...")
    const GROUNDSTATION: &str = r#"{
      "test#DataflowEdgeList": {
        "type": "list",
        "member": { "target": "test#DataflowEdge" },
        "traits": { "smithy.api#length": { "min": 0, "max": 500 } }
      },
      "test#DataflowEdge": {
        "type": "list",
        "member": { "target": "test#ConfigArn" },
        "traits": { "smithy.api#length": { "min": 2, "max": 2 } }
      },
      "test#ConfigArn": {
        "type": "string",
        "traits": {
          "smithy.api#length": { "min": 82, "max": 424 },
          "smithy.api#pattern": "^arn:aws:groundstation:"
        }
      },
      "test#EdgeMap": {
        "type": "map",
        "key": { "target": "smithy.api#String" },
        "value": { "target": "test#DataflowEdge" },
        "traits": { "smithy.api#length": { "min": 1, "max": 4 } }
      },
      "test#Edge": { "type": "structure", "members": {} },
      "test#EdgeStructList": {
        "type": "list",
        "member": { "target": "test#Edge" },
        "traits": { "smithy.api#length": { "min": 2, "max": 2 } }
      }
    }"#;

    fn model() -> HashMap<String, Value> {
        serde_json::from_str(GROUNDSTATION).expect("fixture parses")
    }

    /// Resolve `target` the way the struct emitter does, then capture the
    /// collection member's element constraints.
    ///
    /// `ShapeKind` borrows from the model and from the shape id, so both are
    /// leaked to `'static` rather than threading a lifetime through every
    /// assertion. This runs once per test.
    fn element_constraints_for(target: &str) -> Option<(ShapeKind<'static>, Constraints)> {
        let target: &'static str = Box::leak(target.to_string().into_boxed_str());
        let model: &'static HashMap<String, Value> = Box::leak(Box::new(model()));
        let member: &'static Value = Box::leak(Box::new(
            serde_json::from_str(&format!(r#"{{"target":"{target}"}}"#)).expect("member parses"),
        ));
        let kind: &'static ShapeKind<'static> =
            Box::leak(Box::new(classify_shape(target, model).expect("classifies")));
        extract_element_constraints(kind, member, model).map(|(k, c)| (k.clone(), c))
    }

    #[test]
    fn a_scalar_member_has_no_element_constraints() {
        assert!(element_constraints_for("test#ConfigArn").is_none());
    }

    #[test]
    fn a_list_of_scalars_captures_the_element_shapes_constraints() {
        let (kind, c) = element_constraints_for("test#DataflowEdge").expect("captured");
        assert!(matches!(kind, ShapeKind::String), "{kind:?}");
        assert_eq!(c.length, Some((Some(82), Some(424))));
        assert_eq!(c.pattern.as_deref(), Some("^arn:aws:groundstation:"));
    }

    /// The bug this guards: capturing `DataflowEdge`'s `@length(2, 2)` — an
    /// element *count* — as an element constraint makes the walk demand that
    /// every `ConfigArn` be two characters long, so no legal ARN can pass.
    #[test]
    fn a_nested_list_captures_the_leaf_not_the_inner_lists_element_count() {
        let (kind, c) = element_constraints_for("test#DataflowEdgeList").expect("captured");
        assert!(matches!(kind, ShapeKind::String), "{kind:?}");
        assert_eq!(c.length, Some((Some(82), Some(424))));
        assert_eq!(c.pattern.as_deref(), Some("^arn:aws:groundstation:"));
    }

    #[test]
    fn a_map_of_lists_descends_to_the_leaf() {
        let (kind, c) = element_constraints_for("test#EdgeMap").expect("captured");
        assert!(matches!(kind, ShapeKind::String), "{kind:?}");
        assert_eq!(c.length, Some((Some(82), Some(424))));
    }

    /// A structure element is written against its own schema, so its members are
    /// validated by their own member schemas, and a structure carries no
    /// constraint trait of its own — so the descent bottoms out on a shape with
    /// nothing to capture. What matters here is the negative: the enclosing list's
    /// `@length(2, 2)` does not stand in for it.
    #[test]
    fn a_list_of_structures_captures_neither_the_struct_nor_the_lists_bound() {
        assert!(element_constraints_for("test#EdgeStructList").is_none());
    }

    // ---- constraint traits written on the MEMBER ----------------------------
    //
    // A member may carry a constraint trait itself. `Untagged` is a shape with no
    // traits at all, so a member targeting it has nowhere else for a bound to come
    // from; `Bounded` carries shape-level traits, so it exercises precedence.

    const MEMBER_TRAITS: &str = r#"{
      "test#Untagged": { "type": "string" },
      "test#UntaggedList": { "type": "list", "member": { "target": "test#Untagged" } },
      "test#UntaggedInt": { "type": "integer" },
      "test#Bounded": {
        "type": "string",
        "traits": { "smithy.api#length": { "min": 3, "max": 63 } }
      },
      "test#KeyType": {
        "type": "enum",
        "members": {
          "HASH": { "target": "smithy.api#Unit", "traits": { "smithy.api#enumValue": "HASH" } },
          "RANGE": { "target": "smithy.api#Unit", "traits": { "smithy.api#enumValue": "RANGE" } }
        }
      },
      "test#RenamedEnum": {
        "type": "enum",
        "members": {
          "EC_PRIME256_V1": {
            "target": "smithy.api#Unit",
            "traits": { "smithy.api#enumValue": "EC_prime256v1" }
          }
        }
      },
      "test#ValuelessEnum": {
        "type": "enum",
        "members": { "PLAIN": { "target": "smithy.api#Unit" } }
      },
      "test#LegacyEnum": {
        "type": "string",
        "traits": { "smithy.api#enum": [{ "value": "ON" }, { "value": "OFF" }] }
      }
    }"#;

    fn member_constraints(member_json: &str) -> Constraints {
        let model: HashMap<String, Value> =
            serde_json::from_str(MEMBER_TRAITS).expect("fixture parses");
        let member: Value = serde_json::from_str(member_json).expect("member parses");
        extract_constraints(&member, &model)
    }

    /// The defect: `extract_constraints` read only the target shape, so a bound
    /// written on the member was dropped and nothing enforced it.
    #[test]
    fn a_member_length_is_captured_when_the_target_shape_has_none() {
        let c = member_constraints(
            r#"{"target":"test#Untagged","traits":{"smithy.api#length":{"min":1,"max":8}}}"#,
        );
        assert_eq!(c.length, Some((Some(1), Some(8))));
    }

    /// An element *count* on a collection member, which is where this
    /// exposure concentrates.
    #[test]
    fn a_member_length_on_a_collection_is_captured() {
        let c = member_constraints(
            r#"{"target":"test#UntaggedList","traits":{"smithy.api#length":{"max":25}}}"#,
        );
        assert_eq!(c.length, Some((None, Some(25))));
    }

    #[test]
    fn a_member_range_pattern_and_unique_items_are_captured() {
        let r = member_constraints(
            r#"{"target":"test#UntaggedInt","traits":{"smithy.api#range":{"min":1.0,"max":100.0}}}"#,
        );
        assert_eq!(r.range, Some((Some(1.0), Some(100.0))));

        let p = member_constraints(
            r#"{"target":"test#Untagged","traits":{"smithy.api#pattern":"^a+$"}}"#,
        );
        assert_eq!(p.pattern.as_deref(), Some("^a+$"));

        let u = member_constraints(
            r#"{"target":"test#UntaggedList","traits":{"smithy.api#uniqueItems":{}}}"#,
        );
        assert!(u.unique_items);
    }

    /// Precedence, deliberately: where BOTH declare the same kind the shape wins,
    /// so reading member traits cannot stop enforcing a bound the walk enforces
    /// today. The member here narrows only the minimum, so preferring it would
    /// drop the shape's maximum — the one direction that newly ACCEPTS a request.
    #[test]
    fn a_shape_length_wins_over_a_conflicting_member_length() {
        let c = member_constraints(
            r#"{"target":"test#Bounded","traits":{"smithy.api#length":{"min":1}}}"#,
        );
        assert_eq!(
            c.length,
            Some((Some(3), Some(63))),
            "the shape's bound must win"
        );
    }

    /// Different KINDS compose rather than conflict: the member contributes the
    /// one the shape does not declare.
    #[test]
    fn a_member_trait_of_a_different_kind_composes_with_the_shapes() {
        let c =
            member_constraints(r#"{"target":"test#Bounded","traits":{"smithy.api#pattern":"^p"}}"#);
        assert_eq!(c.length, Some((Some(3), Some(63))));
        assert_eq!(c.pattern.as_deref(), Some("^p"));
    }

    /// A member carrying only non-constraint traits must capture nothing — the
    /// overwhelmingly common case, so a false positive here would affect every generated crate.
    #[test]
    fn a_member_with_only_unrelated_traits_captures_nothing() {
        let c = member_constraints(
            r#"{"target":"test#Untagged","traits":{"smithy.api#documentation":"d","smithy.api#required":{}}}"#,
        );
        assert!(c.is_empty(), "{c:?}");
    }

    // ---- enum membership ----------------------------------------------------

    /// The defect: an `enum` shape's membership was captured from nowhere, so an
    /// illegal value reached the handler. Note the fixture shape declares NO
    /// shape-level `traits` — which is the usual form, and why the capture cannot
    /// live behind the `traits` bail-out.
    #[test]
    fn an_enum_shape_captures_its_members_as_values() {
        let c = member_constraints(r#"{"target":"test#KeyType"}"#);
        assert_eq!(
            c.enum_values,
            Some(vec!["HASH".to_string(), "RANGE".to_string()])
        );
    }

    /// `@enumValue` is the wire string and is often NOT the member name. Capturing
    /// the name would reject the only value the service accepts.
    #[test]
    fn an_enum_captures_the_enum_value_not_the_member_name() {
        let c = member_constraints(r#"{"target":"test#RenamedEnum"}"#);
        assert_eq!(c.enum_values, Some(vec!["EC_prime256v1".to_string()]));
    }

    /// A member declaring no `@enumValue` falls back to its name.
    #[test]
    fn an_enum_member_without_a_value_falls_back_to_its_name() {
        let c = member_constraints(r#"{"target":"test#ValuelessEnum"}"#);
        assert_eq!(c.enum_values, Some(vec!["PLAIN".to_string()]));
    }

    /// The older `string` + `@enum` spelling still works and lands in the same
    /// field, so one `EnumTrait` covers both.
    #[test]
    fn the_legacy_string_enum_spelling_still_captures() {
        let c = member_constraints(r#"{"target":"test#LegacyEnum"}"#);
        assert_eq!(
            c.enum_values,
            Some(vec!["ON".to_string(), "OFF".to_string()])
        );
    }

    /// A member bound to a content-negotiated HTTP header must NOT carry the enum set:
    /// `Accept-Encoding: gzip, deflate` is legal HTTP and does request gzip, so
    /// whole-value equality against `[gzip]` would refuse a request no HTTP front end
    /// could refuse.
    #[test]
    fn a_negotiated_header_member_drops_the_enum_set() {
        for header in [
            "Accept-Encoding",
            "accept",
            "Content-Type",
            "Accept-Language",
        ] {
            let c = member_constraints(&format!(
                r#"{{"target":"test#KeyType","traits":{{"smithy.api#httpHeader":"{header}"}}}}"#
            ));
            assert!(c.enum_values.is_none(), "{header} kept the set: {c:?}");
        }
    }

    /// `x-amz-checksum-mode` is modeled as `[ENABLED]`, but the AWS CLI's CRT S3 client
    /// sends `enabled` and S3 serves it, so the set is kept and matched without regard to
    /// case. Any header casing selects it, and a co-declared bound still applies.
    #[test]
    fn the_checksum_mode_header_member_matches_its_enum_set_ignoring_case() {
        for header in ["x-amz-checksum-mode", "X-Amz-Checksum-Mode"] {
            let c = member_constraints(&format!(
                r#"{{"target":"test#KeyType","traits":{{"smithy.api#httpHeader":"{header}"}}}}"#
            ));
            assert!(c.enum_values.is_some(), "{header} lost the set: {c:?}");
            assert!(c.enum_ignores_case, "{header} matched exactly: {c:?}");
        }

        let bounded = member_constraints(
            r#"{"target":"test#KeyType","traits":{"smithy.api#httpHeader":"x-amz-checksum-mode","smithy.api#length":{"min":1,"max":9}}}"#,
        );
        assert!(bounded.enum_values.is_some(), "{bounded:?}");
        assert!(bounded.enum_ignores_case, "{bounded:?}");
        assert_eq!(bounded.length, Some((Some(1), Some(9))), "{bounded:?}");
    }

    /// `x-amz-sdk-checksum-algorithm` is sent in lower case by the AWS SDK for Swift and
    /// S3 serves it, so its set is kept and matched without regard to case, under any
    /// header casing.
    #[test]
    fn the_sdk_checksum_algorithm_header_member_matches_its_enum_set_ignoring_case() {
        for header in ["x-amz-sdk-checksum-algorithm", "X-Amz-Sdk-Checksum-Algorithm"] {
            let c = member_constraints(&format!(
                r#"{{"target":"test#KeyType","traits":{{"smithy.api#httpHeader":"{header}"}}}}"#
            ));
            assert!(c.enum_values.is_some(), "{header} lost the set: {c:?}");
            assert!(c.enum_ignores_case, "{header} matched exactly: {c:?}");
        }
    }

    /// Case-insensitive matching is for those headers alone: any other header, and a
    /// member bound to none, matches its set exactly.
    #[test]
    fn only_the_client_cased_header_matches_ignoring_case() {
        for member in [
            r#"{"target":"test#KeyType","traits":{"smithy.api#httpHeader":"x-amz-key-type"}}"#,
            r#"{"target":"test#KeyType"}"#,
        ] {
            let c = member_constraints(member);
            assert!(c.enum_values.is_some(), "{c:?}");
            assert!(!c.enum_ignores_case, "{c:?}");
        }
    }

    /// The emitted trait is the case-insensitive constructor only when asked for.
    #[test]
    fn an_ignore_case_enum_emits_the_ignoring_constructor() {
        let mut c = Constraints::default();
        c.enum_values = Some(vec!["ENABLED".to_string()]);
        let exact = emit_constraint_trait_inserts(&c, &ShapeKind::Enum);
        assert!(
            !exact[0].to_string().contains("new_ignoring_case"),
            "{exact:?}"
        );
        c.enum_ignores_case = true;
        let loose = emit_constraint_trait_inserts(&c, &ShapeKind::Enum);
        assert!(
            loose[0].to_string().contains("new_ignoring_case"),
            "{loose:?}"
        );
    }

    /// The suppression is scoped to enum membership and to those headers only: a
    /// different header keeps the set, and a co-declared bound still applies.
    #[test]
    fn the_negotiated_header_suppression_is_narrow() {
        let other = member_constraints(
            r#"{"target":"test#KeyType","traits":{"smithy.api#httpHeader":"x-amz-key-type"}}"#,
        );
        assert!(other.enum_values.is_some(), "{other:?}");

        let bounded = member_constraints(
            r#"{"target":"test#KeyType","traits":{"smithy.api#httpHeader":"Accept","smithy.api#length":{"min":1,"max":9}}}"#,
        );
        assert!(bounded.enum_values.is_none(), "{bounded:?}");
        assert_eq!(bounded.length, Some((Some(1), Some(9))), "{bounded:?}");
    }

    /// An `enum` kind must emit the trait, or capture is inert. Guards the insert
    /// gate against admitting `ShapeKind::String` alone.
    #[test]
    fn an_enum_kind_emits_an_enum_trait_insert() {
        let c = member_constraints(r#"{"target":"test#KeyType"}"#);
        let inserts = emit_constraint_trait_inserts(&c, &ShapeKind::Enum);
        assert_eq!(inserts.len(), 1, "{inserts:?}");
        assert!(inserts[0].to_string().contains("EnumTrait"));
    }

    /// A non-enum, non-string kind must NOT get an `EnumTrait`, so a value set that
    /// somehow reached a list member cannot be applied to the collection itself.
    #[test]
    fn a_list_kind_gets_no_enum_trait() {
        let mut c = Constraints::default();
        c.enum_values = Some(vec!["A".to_string()]);
        let list = ShapeKind::List {
            element: Box::new(ShapeKind::String),
        };
        let inserts = emit_constraint_trait_inserts(&c, &list);
        assert!(inserts.is_empty(), "{inserts:?}");
    }

    #[test]
    fn unwrap_box_strips_the_boxed_type() {
        let boxed = ty("Box<Expression>");
        let inner = unwrap_box(&boxed).expect("Box<T> unwraps");
        assert_eq!(rendered(inner), "Expression");
    }

    #[test]
    fn unwrap_box_accepts_a_fully_qualified_box_path() {
        // The path is matched on its LAST segment, so a qualified `Box` still
        // unwraps — the generator may emit either spelling.
        let boxed = ty("::std::boxed::Box<Expression>");
        let inner = unwrap_box(&boxed).expect("qualified Box unwraps");
        assert_eq!(rendered(inner), "Expression");
    }

    #[test]
    fn unwrap_box_rejects_types_that_are_not_a_box() {
        // Negative cases: every other type this generator emits must pass
        // through untouched, or a non-recursive member would be mis-read.
        for src in [
            "Expression",
            "Option<Expression>",
            "Vec<Expression>",
            "String",
            "std::collections::HashMap<String, Expression>",
        ] {
            assert!(
                unwrap_box(&ty(src)).is_none(),
                "`{src}` must not be treated as a Box"
            );
        }
    }

    #[test]
    fn unwrap_box_rejects_a_box_without_type_arguments() {
        // A bare `Box` (no `<T>`) carries no boxed type to read.
        assert!(unwrap_box(&ty("Box")).is_none());
    }

    /// A `Box<T>` structure member reads the BOXED type and re-boxes the value.
    ///
    /// `Box<T>` does not implement `DeserializableStruct`, so naming it as the
    /// `Self` type of that trait call is an `E0277`. This is what lets a
    /// self-referential shape be given finite size with a `Box` and still
    /// deserialize.
    #[test]
    fn emit_read_boxes_a_boxed_structure_member() {
        let out = emit_read(
            &ShapeKind::Structure,
            &ty("Box<Expression>"),
            &quote! { &Self::M_NOT },
            &quote! { __d },
        )
        .expect("emits")
        .to_string();
        assert!(out.contains("Box :: new"), "value must be re-boxed: {out}");
        assert!(
            out.contains("< Expression as"),
            "the BOXED type must be the trait Self type, not Box<_>: {out}"
        );
        assert!(
            !out.contains("< Box < Expression > as"),
            "must never name Box<_> as the DeserializableStruct Self type: {out}"
        );
    }

    /// The unboxed path is unchanged: a plain structure member still reads
    /// directly, with no `Box::new` wrapper.
    #[test]
    fn emit_read_leaves_an_unboxed_structure_member_alone() {
        let out = emit_read(
            &ShapeKind::Structure,
            &ty("Expression"),
            &quote! { &Self::M_FILTER },
            &quote! { __d },
        )
        .expect("emits")
        .to_string();
        assert!(
            out.contains("< Expression as"),
            "unboxed member must read directly: {out}"
        );
        assert!(
            !out.contains("Box :: new"),
            "unboxed member must NOT be re-boxed: {out}"
        );
    }

    /// A boxed `union` member takes the same path as a boxed structure.
    #[test]
    fn emit_read_boxes_a_boxed_union_member() {
        let out = emit_read(
            &ShapeKind::Union,
            &ty("Box<Predicate>"),
            &quote! { &Self::M_P },
            &quote! { __d },
        )
        .expect("emits")
        .to_string();
        assert!(out.contains("Box :: new"), "{out}");
        assert!(out.contains("< Predicate as"), "{out}");
    }

    #[test]
    fn an_operations_auth_schemes_come_from_its_own_auth_else_the_services() {
        let service =
            serde_json::json!({"traits": {"smithy.api#httpBearerAuth": {}, "aws.auth#sigv4": {}}});
        let listing = serde_json::json!({"traits": {"smithy.api#auth": ["aws.auth#sigv4a", "aws.auth#sigv4"]}});
        let ranked =
            serde_json::json!({"traits": {"smithy.api#auth": ["smithy.api#httpBearerAuth"]}});
        #[rustfmt::skip]
        let cases = [
            ("operation @auth wins, order kept", &listing, &service, vec!["aws.auth#sigv4a", "aws.auth#sigv4"]),
            ("service @auth", &serde_json::json!({}), &ranked, vec!["smithy.api#httpBearerAuth"]),
            ("service schemes, alphabetical", &serde_json::json!({}), &service, vec!["aws.auth#sigv4", "smithy.api#httpBearerAuth"]),
            ("no known scheme fails closed", &serde_json::json!({}), &serde_json::json!({"traits": {"aws.auth#cognitoUserPools": {}}}), vec!["aws.auth#sigv4"]),
        ];
        for (name, operation, service, expected) in cases {
            assert_eq!(
                operation_auth_schemes(operation, service),
                expected,
                "{name}"
            );
        }
        let optional = serde_json::json!({"traits": {"smithy.api#optionalAuth": {}}});
        assert!(operation_auth_schemes(&optional, &service).is_empty());
        let unauthenticated_service = serde_json::json!({"traits": {"smithy.api#auth": []}});
        assert!(
            operation_auth_schemes(&serde_json::json!({}), &unauthenticated_service).is_empty()
        );
    }
}
