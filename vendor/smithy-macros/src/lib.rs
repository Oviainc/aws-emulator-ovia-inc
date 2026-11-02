// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
use proc_macro::TokenStream;
use quote::quote;
use std::collections::{HashMap, HashSet};
use syn::{parse_macro_input, Data, DeriveInput, Fields, GenericArgument, PathArguments, Type};

mod client;
mod codegen;

// =============================================================================
// The single #[smithy(...)] attribute macro
// =============================================================================

/// Unified attribute macro replacing all Smithy derives.
///
/// Usage:
/// ```ignore
/// // Set the model path for this module once.
/// smithy_macros::smithy_model!("smithy-model/dynamodb-raw.json");
///
/// // Preferred: use `shape` and let the macro infer behavior from the model.
/// #[smithy(shape = "com.amazonaws.dynamodb#PutItemInput", unsupported = ["Field1", "Field2"])]
/// #[derive(Debug, Clone, Default)]
/// pub struct PutItemInput { ... }
///
/// #[smithy(shape = "com.amazonaws.dynamodb#KeyType")]
/// #[derive(Debug, Clone, Default, PartialEq, Eq)]
/// pub enum KeyType { Hash, Range }
///
/// #[smithy(shape = "com.amazonaws.dynamodb#ResourceNotFoundException")]
/// #[derive(Debug, Clone)]
/// pub struct ResourceNotFoundException { ... }
///
/// // For operation error enums, use the separate `operation_error` macro:
/// #[smithy_macros::operation_error("com.amazonaws.dynamodb#PutItem", extra_errors = ["TableNotFoundException"])]
/// #[derive(Debug)]
/// pub enum PutItemError { ... }
/// ```
///
/// Implementation: this attribute forwards to a local `macro_rules! smithy`
/// that was emitted by [`smithy_model!`] earlier in the module. The
/// declarative macro injects the module's model path and forwards to the
/// real codegen proc-macro [`smithy_codegen!`]. See `smithy_model!` for why
/// this two-step expansion is necessary.
#[proc_macro_attribute]
pub fn smithy(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr: proc_macro2::TokenStream = attr.into();
    let item: proc_macro2::TokenStream = item.into();
    quote! {
        smithy! { (#attr) #item }
    }
    .into()
}

/// Attribute macro for operation error enums.
///
/// Usage:
/// ```ignore
/// #[smithy_macros::operation_error("com.amazonaws.dynamodb#PutItem", extra_errors = ["TableNotFoundException"])]
/// #[derive(Debug)]
/// pub enum PutItemError { ... }
/// ```
///
/// This generates the error dispatcher enum from the operation's `errors`
/// list in the Smithy model. Like `#[smithy(...)]`, it forwards through the
/// local `smithy!` macro emitted by [`smithy_model!`] to inject the model path.
///
/// # `extra_errors` widens the accepted set; it does NOT generate a shape
///
/// `extra_errors` lets a variant through this macro's own consistency check for
/// an error the SERVICE models but this OPERATION does not list in its `errors`
/// — `TableNotFoundException` above is a `com.amazonaws.dynamodb` shape that
/// `PutItem` omits. The struct and its `SCHEMA_*` static must already exist,
/// which for a modeled shape means a `#[smithy(shape = "…")]` elsewhere in the
/// module emitted them.
///
/// It cannot conjure a shape the model never declares. Naming one that does not
/// exist fails to compile on the missing type and the missing `SCHEMA_*` static,
/// not with a diagnostic from this macro — so reach for it only for shapes you
/// can point at in the model. A service whose model declares no client-input
/// error at all therefore has nothing to widen in, and `extra_errors` is not the
/// tool for it.
///
/// # `awsQueryCompatible` support (auto-generated — no implementer action)
///
/// For a service migrated from `awsQuery` to `awsJson` (the
/// `aws.protocols#awsQueryCompatible` trait, e.g. SQS), each error shape may
/// carry an `aws.protocols#awsQueryError` trait pinning its legacy query error
/// `code`. This macro reads that trait straight from the model and emits
/// `OperationError::query_error_code()` returning that code (or `None`). The
/// dispatch layer then surfaces it as the `x-amzn-query-error:
/// <code>;<Sender|Receiver>` response header — but **only** for requests that
/// opted in with the `x-amzn-query-mode: true` header. Implementers write their
/// error enum exactly as usual; the query-compat wiring is entirely generated.
#[proc_macro_attribute]
pub fn operation_error(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr2: proc_macro2::TokenStream = attr.into();
    let item2: proc_macro2::TokenStream = item.into();

    // Parse: first token must be a string literal (the operation shape ID),
    // optionally followed by `, key = value, ...` pairs.
    let mut iter = attr2.into_iter().peekable();

    // Grab the string literal
    let op_id = match iter.next() {
        Some(tok) => tok,
        None => {
            let e = syn::Error::new(
                proc_macro2::Span::call_site(),
                "#[operation_error] requires an operation shape ID as the first argument",
            );
            return emit_codegen_error(e, Some(item2)).into();
        }
    };

    // Skip the comma after the string literal if present
    if let Some(proc_macro2::TokenTree::Punct(p)) = iter.peek() {
        if p.as_char() == ',' {
            iter.next();
        }
    }

    let rest: proc_macro2::TokenStream = iter.collect();
    let trailing = if rest.is_empty() {
        quote! {}
    } else {
        quote! { , #rest }
    };

    quote! {
        smithy! { (error_for = #op_id #trailing) #item2 }
    }
    .into()
}

/// Declare the Smithy model file used by every `#[smithy(...)]` invocation
/// in this module. Call once at the top of the module:
///
/// ```ignore
/// smithy_macros::smithy_model!("smithy-model/kms.json");
/// ```
///
/// # How it works
///
/// `#[smithy(...)]` itself doesn't know the model path — it just expands to
/// `smithy! { (args) item }`, an invocation of a *local* declarative macro.
/// `smithy_model!` is the proc-macro that defines that local declarative
/// macro, baking the path literal into its expansion template:
///
/// ```ignore
/// macro_rules! smithy {
///     ( ( $($args:tt)* ) $($item:tt)* ) => {
///         ::smithy_macros::smithy_codegen! {
///             (model = "smithy-model/kms.json", $($args)*)
///             $($item)*
///         }
///     };
/// }
/// ```
///
/// So a single module ends up with two layers of expansion: the proc-macro
/// attribute `#[smithy(...)]` writes `smithy! { ... }`, then the local
/// `smithy!` rewrites the call with `model = "..."` prepended, then the
/// real `smithy_codegen!` proc-macro does the codegen.
///
/// This avoids hardcoding any per-service default into the framework while
/// keeping every call site as plain `#[smithy(...)]`.
#[proc_macro]
pub fn smithy_model(input: TokenStream) -> TokenStream {
    let model: proc_macro2::TokenStream = input.into();
    // Validate: must be a single string literal, otherwise we get a
    // baffling error inside the generated macro_rules!.
    let parsed: syn::Result<syn::LitStr> = syn::parse2(model.clone());
    if let Err(e) = parsed {
        return emit_codegen_error(e, None).into();
    }
    // Resolve the consumer's name for this crate (default `smithy_macros`)
    // so the emitted `macro_rules!` references it correctly even if the
    // consumer renamed it in Cargo.toml.
    let crate_path = resolve_crate_path();
    quote! {
        macro_rules! smithy {
            ( ( $($__args:tt)* ) $($__item:tt)* ) => {
                #crate_path::smithy_codegen! {
                    (model = #model, $($__args)*)
                    $($__item)*
                }
            };
        }
    }
    .into()
}

/// Resolve the path the consumer uses to refer to *this* crate. Returns
/// `::smithy_macros` by default, or `::<alias>` if the consumer renamed it.
fn resolve_crate_path() -> proc_macro2::TokenStream {
    use proc_macro_crate::FoundCrate;
    match proc_macro_crate::crate_name("amzn-smithy-macros") {
        Ok(FoundCrate::Itself) => quote! { crate },
        Ok(FoundCrate::Name(name)) => {
            let ident = quote::format_ident!("{}", name);
            quote! { ::#ident }
        }
        Err(_) => quote! { ::smithy_macros },
    }
}

/// Internal: do not call directly. The function-like proc macro that
/// performs the actual codegen for `#[smithy(...)]`. Receives
/// `(args...) <item>` where `args` includes the `model = "..."` injected by
/// the local `smithy!` declarative macro emitted by [`smithy_model!`].
#[proc_macro]
pub fn smithy_codegen(input: TokenStream) -> TokenStream {
    let input: proc_macro2::TokenStream = input.into();
    let mut iter = input.into_iter();

    // First token must be a parenthesized group of args.
    let attr_tokens = match iter.next() {
        Some(proc_macro2::TokenTree::Group(g))
            if g.delimiter() == proc_macro2::Delimiter::Parenthesis =>
        {
            g.stream()
        }
        Some(t) => {
            let e = syn::Error::new(t.span(), "expected `(args...)` group");
            return emit_codegen_error(e, None).into();
        }
        None => {
            let e = syn::Error::new(
                proc_macro2::Span::call_site(),
                "expected `(args...) <item>`",
            );
            return emit_codegen_error(e, None).into();
        }
    };
    let item_tokens: proc_macro2::TokenStream = iter.collect();

    // Save the original item so we can re-emit it on error and keep
    // rust-analyzer happy.
    let item_tokens_for_error = item_tokens.clone();

    match smithy_codegen_impl(attr_tokens, item_tokens) {
        Ok(tokens) => tokens,
        Err(e) => emit_codegen_error(e, Some(item_tokens_for_error)).into(),
    }
}

// =============================================================================
// Diagnostics: one clear root error, no cascade
// =============================================================================
//
// A single mistake in a model — a wrong `smithy_model!` path, malformed JSON, a
// renamed/missing shape — is observed by EVERY `#[smithy(...)]` invocation in
// the module, and a service module routinely has 10-50 of them all sharing one
// model. Left unchecked, each invocation independently emits the SAME
// `compile_error!`, and each re-emits its bare item WITHOUT the generated impls,
// so the one useful diagnostic drowns under a wall of duplicate errors and
// downstream E0599/E0061 "method not found" noise.
//
// `emit_codegen_error` collapses that wall: the first invocation to hit a given
// root cause emits the full `compile_error!` (placed FIRST, so it surfaces
// before the re-emitted item), and identical follow-on diagnostics in the same
// compilation are suppressed. The original item is always re-emitted so its
// type still resolves and we don't trade one cascade for a "cannot find type"
// cascade.

/// Process-global set of root diagnostics already surfaced in this compilation.
/// rustc loads the proc-macro dylib once per crate compilation and drives every
/// expansion from that single process (the same assumption `MODEL_CACHE` relies
/// on), so this dedupes within one crate build and resets for the next.
static EMITTED_DIAGNOSTICS: std::sync::OnceLock<std::sync::Mutex<HashSet<String>>> =
    std::sync::OnceLock::new();

/// Render a codegen failure into output tokens, deduplicating identical root
/// causes across sibling invocations in the same compilation. `item` is the
/// original user item to re-emit (so its type still resolves); pass `None` when
/// there is no item to preserve.
fn emit_codegen_error(
    err: syn::Error,
    item: Option<proc_macro2::TokenStream>,
) -> proc_macro2::TokenStream {
    let seen = EMITTED_DIAGNOSTICS.get_or_init(|| std::sync::Mutex::new(HashSet::new()));
    render_codegen_error(err, item, seen)
}

/// Core of [`emit_codegen_error`], split out with an injectable `seen` set so it
/// is unit-testable without touching process-global state. The first time a
/// given diagnostic message is seen it is surfaced in full; every identical
/// follow-on is suppressed (only the re-emitted item, if any, comes through).
fn render_codegen_error(
    err: syn::Error,
    item: Option<proc_macro2::TokenStream>,
    seen: &std::sync::Mutex<HashSet<String>>,
) -> proc_macro2::TokenStream {
    let first = seen.lock().unwrap().insert(err.to_string());
    build_error_output(&err, item.as_ref(), first)
}

/// Build the token output for a failed expansion. The `compile_error!` is
/// emitted FIRST (so the actionable message is not buried after the item) and
/// only when `emit_diagnostic` is true — duplicates of an already-surfaced root
/// cause emit just the re-emitted item, with no follow-on `compile_error!`.
fn build_error_output(
    err: &syn::Error,
    item: Option<&proc_macro2::TokenStream>,
    emit_diagnostic: bool,
) -> proc_macro2::TokenStream {
    let mut out = proc_macro2::TokenStream::new();
    if emit_diagnostic {
        out.extend(err.to_compile_error());
    }
    if let Some(item) = item {
        out.extend(item.clone());
    }
    out
}

/// Compose an actionable "not found" diagnostic: the missing id, up to three
/// nearest shape ids actually present in the model ("did you mean …?"), and a
/// reminder to check the `smithy_model!("…")` path. `kind` labels the lookup
/// (e.g. "Shape", "Operation", "Service shape").
fn shape_not_found_message(
    kind: &str,
    shape_id: &str,
    model: &HashMap<String, serde_json::Value>,
) -> String {
    let mut msg = format!("{} '{}' not found in Smithy model", kind, shape_id);
    let suggestions = nearest_shape_ids(shape_id, model.keys());
    if !suggestions.is_empty() {
        msg.push_str("\n  help: did you mean one of: ");
        msg.push_str(&suggestions.join(", "));
        msg.push('?');
    }
    msg.push_str(
        "\n  note: verify the shape ID and the `smithy_model!(\"…\")` path for this module",
    );
    msg
}

/// Resolve a list member's XML layout: whether it is `@xmlFlattened`, and the
/// element name for its items in the wrapped layout.
///
/// `@xmlFlattened` is legal in two places and real AWS models use both: S3 puts
/// it on the LIST SHAPE (`ObjectIdentifierList` is `"flattened": true`) while
/// naming the element with `@xmlName` on the STRUCTURE MEMBER
/// (`Delete.Objects` is `"locationName": "Object"`). Checking only the member
/// would miss every shape-level declaration, so both are consulted.
///
/// The item name comes from the `@xmlName` on the list's own member and is only
/// meaningful when the list is NOT flattened — a flattened list's repeated
/// elements are named by the structure member (see `XmlListTrait`).
fn xml_list_layout<'a>(
    smithy_member: &'a serde_json::Value,
    model: &'a HashMap<String, serde_json::Value>,
) -> (bool, Option<&'a str>) {
    let list_shape = smithy_member
        .get("target")
        .and_then(|t| t.as_str())
        .and_then(|target| model.get(target));

    let item_name = list_shape
        .and_then(|s| s.get("member"))
        .and_then(|m| m.get("traits"))
        .and_then(|t| t.get("smithy.api#xmlName"))
        .and_then(|v| v.as_str());

    let has_flattened = |value: Option<&serde_json::Value>| {
        value
            .and_then(|v| v.get("traits"))
            .and_then(|t| t.as_object())
            .is_some_and(|t| t.contains_key("smithy.api#xmlFlattened"))
    };
    let flattened = has_flattened(Some(smithy_member)) || has_flattened(list_shape);

    (flattened, item_name)
}

/// Whether a map member is `@xmlFlattened`.
///
/// `@xmlFlattened` is legal on the map member and on the map shape itself, so
/// both are consulted — the same two placements the list helper checks. A
/// flattened map needs no item name: its key and value element names come from
/// the map's key/value members at runtime (`Schema::key()`/`Schema::member()`),
/// and the repeated element is the map member's own name, which `xml_name`
/// already carries.
fn xml_map_layout(
    smithy_member: &serde_json::Value,
    model: &HashMap<String, serde_json::Value>,
) -> bool {
    let map_shape = smithy_member
        .get("target")
        .and_then(|t| t.as_str())
        .and_then(|target| model.get(target));

    let has_flattened = |value: Option<&serde_json::Value>| {
        value
            .and_then(|v| v.get("traits"))
            .and_then(|t| t.as_object())
            .is_some_and(|t| t.contains_key("smithy.api#xmlFlattened"))
    };

    has_flattened(Some(smithy_member)) || has_flattened(map_shape)
}

/// Build the key/value (map) or element (list) sub-member schemas for a
/// structure member and the setter chain that attaches them to that member's
/// schema.
///
/// A map or list member schema has to carry the schemas of the collection's
/// inner members, because the XML codec reads the key/value element names from
/// them (`Schema::key()`/`Schema::member()`) on BOTH the serialize and
/// deserialize paths. Without them the codec can only fall back to the Smithy
/// defaults (`key`/`value`/`member`), which silently drops a modeled `@xmlName`
/// on the key or value element — producing a response the client cannot parse
/// and failing to read a request that uses the modeled names.
///
/// Returns the extra `const` sub-schema declarations to emit beside the member
/// schema, and the `.with_map_members(..)` / `.with_list_member(..)` chain to
/// append to it. Returns `(vec![], empty)` for a non-collection member or when
/// the target shape cannot be resolved (the codec then keeps its defaults).
///
/// `member_static` is the ident of the member schema these sub-schemas belong
/// to; the sub-schema idents are derived from it so they are unique within the
/// surrounding scope. `shape_id_prefix` is the `namespace#Local$member` string
/// used to mint synthetic shape ids for the sub-members.
fn map_list_member_schema(
    member_static: &proc_macro2::Ident,
    shape_id_prefix: &str,
    namespace: &str,
    smithy_member: &serde_json::Value,
    model: &HashMap<String, serde_json::Value>,
) -> (Vec<proc_macro2::TokenStream>, proc_macro2::TokenStream) {
    let target_shape = smithy_member
        .get("target")
        .and_then(|t| t.as_str())
        .and_then(|target| model.get(target));
    let Some(target_shape) = target_shape else {
        return (Vec::new(), quote! {});
    };
    let shape_type = target_shape.get("type").and_then(|t| t.as_str());

    // The modeled `@xmlName` on a collection's inner member, if any.
    let inner_xml_name = |slot: &str| -> Option<String> {
        target_shape
            .get(slot)
            .and_then(|m| m.get("traits"))
            .and_then(|t| t.get("smithy.api#xmlName"))
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    };

    // Emit a single sub-member schema const. `member_name` is the Smithy
    // default name for the slot (`key`/`value`/`member`); the modeled
    // `@xmlName`, when present, is chained so the codec renames the element.
    let sub_member =
        |ident: &proc_macro2::Ident, slot: &str, member_name: &str| -> proc_macro2::TokenStream {
            let xml_chain = match inner_xml_name(slot) {
                Some(xn) => quote! { .with_xml_name(#xn) },
                None => quote! {},
            };
            let local = format!("{shape_id_prefix}${member_name}");
            quote! {
                const #ident: ::aws_smithy_schema::Schema =
                    ::aws_smithy_schema::Schema::new_member(
                        ::aws_smithy_schema::ShapeId::from_static(#local, #namespace, #member_name),
                        // The sub-member's shape type is unused by the element-name
                        // lookup the codec performs; `String` is a safe placeholder.
                        ::aws_smithy_schema::ShapeType::String,
                        #member_name,
                        0usize,
                    )#xml_chain;
            }
        };

    match shape_type {
        Some("map") => {
            let key_ident = quote::format_ident!("{}_KEY", member_static);
            let value_ident = quote::format_ident!("{}_VALUE", member_static);
            let decls = vec![
                sub_member(&key_ident, "key", "key"),
                sub_member(&value_ident, "value", "value"),
            ];
            let chain = quote! { .with_map_members(&Self::#key_ident, &Self::#value_ident) };
            (decls, chain)
        }
        Some("list") => {
            let elem_ident = quote::format_ident!("{}_MEMBER", member_static);
            let decls = vec![sub_member(&elem_ident, "member", "member")];
            let chain = quote! { .with_list_member(&Self::#elem_ident) };
            (decls, chain)
        }
        _ => (Vec::new(), quote! {}),
    }
}

/// The `@xmlName` setter chain for a member schema, or an empty chain when the
/// member has no `@xmlName`.
///
/// `@xmlName` renames the XML element a member occupies. It has its own setter
/// and accessor on `aws-smithy-schema` (`with_xml_name`/`xml_name`), and the
/// restXml/awsQuery codecs read `xml_name()` on both the serialize and
/// deserialize paths, so baking it onto the schema renames the element on the
/// wire. `@xmlName` is independent of `@jsonName`: a member may carry one, the
/// other, or both, and each reaches only its own codec.
fn xml_name_chain(
    member_traits: Option<&serde_json::Map<String, serde_json::Value>>,
) -> proc_macro2::TokenStream {
    match member_traits
        .and_then(|t| t.get("smithy.api#xmlName"))
        .and_then(|v| v.as_str())
    {
        Some(name) => quote! { .with_xml_name(#name) },
        None => quote! {},
    }
}

/// The `@jsonName` setter chain for a member schema, or an empty chain when the
/// member has no `@jsonName`.
///
/// `@jsonName` renames a member's JSON object key. It has its own setter and
/// accessor on `aws-smithy-schema` (`with_json_name`/`json_name`), and the
/// restJson1/awsJson codecs read `json_name()` on both the serialize and
/// deserialize paths, so baking it onto the schema renames the key on the wire
/// in both directions. It is independent of `@xmlName` (see `xml_name_chain`).
fn json_name_chain(
    member_traits: Option<&serde_json::Map<String, serde_json::Value>>,
) -> proc_macro2::TokenStream {
    match member_traits
        .and_then(|t| t.get("smithy.api#jsonName"))
        .and_then(|v| v.as_str())
    {
        Some(name) => quote! { .with_json_name(#name) },
        None => quote! {},
    }
}

/// A `TraitMap` insert carrying a member's `@ec2QueryName` for the ec2Query
/// input codec, or `None` when the member has no `@ec2QueryName`.
///
/// `@ec2QueryName` renames a member's key in an ec2Query INPUT. Unlike
/// `@xmlName`/`@jsonName` it has no dedicated setter or accessor on
/// `aws-smithy-schema`, so carry it on the member's generic `TraitMap` as a
/// `StringTrait` keyed by its Smithy shape id. The ec2 query deserializer
/// (`member_key`) prefers this over the capitalised member name, keeping it
/// distinct from `@xmlName`, which the model may also set for the awsQuery
/// (non-ec2) layout. The two names differ and must each reach their own codec.
fn ec2_query_name_insert(
    member_traits: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<proc_macro2::TokenStream> {
    let name = member_traits
        .and_then(|t| t.get("smithy.api#ec2QueryName"))
        .and_then(|v| v.as_str())?;
    Some(quote! {
        m.insert(::std::boxed::Box::new(
            ::aws_smithy_schema::StringTrait::new(
                ::aws_smithy_schema::ShapeId::from_static(
                    "smithy.api#ec2QueryName",
                    "smithy.api",
                    "ec2QueryName",
                ),
                #name,
            )
        ));
    })
}

/// Collect an operation's `@required` HTTP header / query bindings as
/// `RequiredBinding` token streams for its `Route` entry.
///
/// These are the request-observable members a REST router can use to tell apart
/// operations that share a method + URI template. Only members that are BOTH
/// `@required` and bound to a header (`@httpHeader`) or a query parameter
/// (`@httpQuery`) qualify: an optional member carries no obligation, and a
/// `@httpLabel` is already implied by the matched path. The wire name comes from
/// the trait's value (e.g. `x-amz-copy-source`), falling back to the member name
/// when the trait carries no explicit name.
fn required_http_bindings(
    op_shape: &serde_json::Value,
    model: &HashMap<String, serde_json::Value>,
) -> Vec<proc_macro2::TokenStream> {
    let Some(input) = op_shape
        .get("input")
        .and_then(|i| i.get("target"))
        .and_then(|t| t.as_str())
        .and_then(|id| model.get(id))
    else {
        return Vec::new();
    };
    let Some(members) = input.get("members").and_then(|m| m.as_object()) else {
        return Vec::new();
    };

    let mut bindings = Vec::new();
    for (member_name, member) in members {
        let Some(traits) = member.get("traits").and_then(|t| t.as_object()) else {
            continue;
        };
        if !traits.contains_key("smithy.api#required") {
            continue;
        }
        if let Some(header) = traits.get("smithy.api#httpHeader") {
            let name = header.as_str().unwrap_or(member_name);
            bindings.push(quote! {
                ::aws_smithy_schema_server::RequiredBinding::Header(#name)
            });
        } else if let Some(query) = traits.get("smithy.api#httpQuery") {
            let name = query.as_str().unwrap_or(member_name);
            bindings.push(quote! {
                ::aws_smithy_schema_server::RequiredBinding::Query(#name)
            });
        }
    }
    bindings
}

/// Rank the model's shape ids by closeness to `wanted` and return up to three
/// plausible typo/namespace mistakes. A shape sharing the same local name (the
/// part after '#') always ranks first — the single most common real mistake is
/// a wrong namespace. Otherwise rank by Levenshtein distance within a small
/// budget so unrelated ids are never suggested.
fn nearest_shape_ids<'a, I>(wanted: &str, candidates: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a String>,
{
    let wanted_local = local_name(wanted);
    let budget = std::cmp::max(2, wanted.len() / 3);
    let mut same_local: Vec<&str> = Vec::new();
    let mut scored: Vec<(usize, &str)> = Vec::new();
    for cand in candidates {
        if cand == wanted {
            continue;
        }
        if !wanted_local.is_empty() && local_name(cand).eq_ignore_ascii_case(wanted_local) {
            same_local.push(cand);
            continue;
        }
        let d = levenshtein(wanted, cand);
        if d <= budget {
            scored.push((d, cand));
        }
    }
    same_local.sort_unstable();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
    same_local
        .into_iter()
        .chain(scored.into_iter().map(|(_, c)| c))
        .take(3)
        .map(str::to_string)
        .collect()
}

/// The Smithy shape's local name — everything after the last '#', or the whole
/// id when there is no namespace separator.
fn local_name(shape_id: &str) -> &str {
    shape_id.rsplit('#').next().unwrap_or(shape_id)
}

/// Classic two-row iterative Levenshtein edit distance. Short shape ids make
/// this trivially cheap and it keeps the crate dependency-free.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] =
                std::cmp::min(std::cmp::min(curr[j] + 1, prev[j + 1] + 1), prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

fn smithy_codegen_impl(
    attr: proc_macro2::TokenStream,
    item: proc_macro2::TokenStream,
) -> syn::Result<TokenStream> {
    let mut args = SmithyArgs::parse(attr)?;
    let item_ast: syn::Item = syn::parse2(item)?;

    // If `shape = "..."` is set, resolve it into the appropriate legacy key
    // based on the model's shape type, traits, and the Rust item kind.
    if args.shape.is_some() && args.shape_key() == Some("shape") {
        let shape_id = args.shape.as_ref().unwrap().clone();
        let model_path = args.model_path()?.to_string();
        let model = load_smithy_model(&model_path).map_err(|e| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("Failed to load Smithy model: {}", e),
            )
        })?;
        let shape = model.get(&shape_id).ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                shape_not_found_message("Shape", &shape_id, &model),
            )
        })?;
        let shape_type = shape.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let traits = shape.get("traits").and_then(|t| t.as_object());
        let is_rust_enum = matches!(&item_ast, syn::Item::Enum(_));

        match (shape_type, is_rust_enum) {
            ("enum", true) => {
                args.enum_for = args.shape.take();
            }
            ("union", true) => {
                args.union_for = args.shape.take();
            }
            ("operation", true) => {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!("Shape '{}' is an operation. Use #[smithy_macros::operation_error(\"...\")] for operation error enums.", shape_id),
                ));
            }
            ("structure", false) => {
                let has_error = traits
                    .map(|t| t.contains_key("smithy.api#error"))
                    .unwrap_or(false);
                if has_error {
                    args.error = args.shape.take();
                } else {
                    // input/output/structure all take the same codegen path
                    args.structure = args.shape.take();
                }
            }
            ("enum", false) => {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!(
                        "Shape '{}' is an enum but the Rust item is a struct. Use an enum.",
                        shape_id
                    ),
                ));
            }
            ("union", false) => {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!(
                        "Shape '{}' is a union but the Rust item is a struct. Use an enum.",
                        shape_id
                    ),
                ));
            }
            ("operation", false) => {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!("Shape '{}' is an operation. Use #[smithy_macros::operation_error(\"...\")] for operation error enums.", shape_id),
                ));
            }
            ("structure", true) => {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!(
                        "Shape '{}' is a structure but the Rust item is an enum. Use a struct.",
                        shape_id
                    ),
                ));
            }
            _ => {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!(
                        "Shape '{}' has unsupported type '{}' for #[smithy(shape = ...)]",
                        shape_id, shape_type
                    ),
                ));
            }
        }
    }

    match &item_ast {
        syn::Item::Struct(item_struct) => smithy_on_struct(args, item_struct),
        syn::Item::Enum(item_enum) => smithy_on_enum(args, item_enum),
        _ => Err(syn::Error::new_spanned(
            quote! {},
            "#[smithy(...)] can only be applied to structs or enums",
        )),
    }
}

// =============================================================================
// Argument parsing
// =============================================================================

/// Parsed arguments from `#[smithy(...)]`.
struct SmithyArgs {
    /// Unified shape ID parameter. When set, the macro infers behavior from
    /// the model's shape type + Rust item kind:
    /// - model `enum` + Rust `enum` → enum codegen
    /// - model `union` + Rust `enum` → union codegen
    /// - model `operation` + Rust `enum` → error_for codegen
    /// - model `structure` + `@error` trait + Rust `struct` → error variant
    /// - model `structure` + `@input`/`@output`/neither + Rust `struct` → struct codegen
    shape: Option<String>,
    input: Option<String>,
    output: Option<String>,
    structure: Option<String>,
    enum_for: Option<String>,
    /// Smithy `union` shape ID. Generates `SerializableStruct` /
    /// `DeserializableStruct` impls that round-trip a single active variant
    /// through the existing JSON codec — `{"S":"hello"}` etc.
    union_for: Option<String>,
    error: Option<String>,
    error_for: Option<String>,
    unsupported: Vec<String>,
    /// Extra error variants not declared in the Smithy model but allowed
    /// on the enum (e.g. `ValidationException` added by the emulator).
    extra_errors: Vec<String>,
    /// Per-variant Rust→Smithy renames, parsed from
    /// `rename = ["RustVariant=SmithyMember", ...]`. Used by `union_for`
    /// when the engine's enum variants don't follow the default
    /// SCREAMING_SNAKE_CASE→PascalCase mapping (e.g. `StringSet` → `SS`).
    rename: HashMap<String, String>,
    /// Workspace-relative path to the Smithy model JSON. Required: must be
    /// supplied either by `smithy_model!("...")` (which injects it) or by
    /// passing `model = "..."` directly on the attribute.
    model: Option<String>,
    /// Fields whose constraint validation should be skipped during
    /// deserialization, allowing the handler to produce a more specific error.
    skip_validation: Vec<String>,
}

impl SmithyArgs {
    fn parse(tokens: proc_macro2::TokenStream) -> syn::Result<Self> {
        use syn::parse::Parser;
        use syn::{Expr, ExprArray, ExprLit, Lit, Meta, MetaNameValue};

        let mut args = SmithyArgs {
            shape: None,
            input: None,
            output: None,
            structure: None,
            enum_for: None,
            union_for: None,
            error: None,
            error_for: None,
            unsupported: Vec::new(),
            extra_errors: Vec::new(),
            rename: HashMap::new(),
            model: None,
            skip_validation: Vec::new(),
        };

        let parser = syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated;
        let metas = parser.parse2(tokens)?;

        for meta in metas {
            match meta {
                Meta::NameValue(MetaNameValue { path, value, .. }) => {
                    let key = path
                        .get_ident()
                        .ok_or_else(|| syn::Error::new_spanned(&path, "expected identifier"))?
                        .to_string();

                    match key.as_str() {
                        "input" | "output" | "structure" | "enum_for" | "union_for" | "error"
                        | "error_for" | "model" | "shape" => {
                            let s = expr_to_string(&value)?;
                            match key.as_str() {
                                "input" => args.input = Some(s),
                                "output" => args.output = Some(s),
                                "structure" => args.structure = Some(s),
                                "enum_for" => args.enum_for = Some(s),
                                "union_for" => args.union_for = Some(s),
                                "error" => args.error = Some(s),
                                "error_for" => args.error_for = Some(s),
                                "model" => args.model = Some(s),
                                "shape" => args.shape = Some(s),
                                _ => unreachable!(),
                            }
                        }
                        "unsupported" | "extra_errors" | "skip_validation" => {
                            // Must be an array: unsupported = ["A", "B"]
                            let target = if key == "unsupported" {
                                &mut args.unsupported
                            } else if key == "extra_errors" {
                                &mut args.extra_errors
                            } else {
                                &mut args.skip_validation
                            };
                            match &value {
                                Expr::Array(ExprArray { elems, .. }) => {
                                    for elem in elems {
                                        target.push(expr_to_string(elem)?);
                                    }
                                }
                                Expr::Lit(ExprLit {
                                    lit: Lit::Str(_), ..
                                }) => {
                                    return Err(syn::Error::new_spanned(
                                        &value,
                                        format!("use `{key} = [\"single\"]` for a single value, or omit entirely"),
                                    ));
                                }
                                _ => {
                                    return Err(syn::Error::new_spanned(
                                        &value,
                                        format!("{key} must be a list: {key} = [\"A\", \"B\"]"),
                                    ));
                                }
                            }
                        }
                        "rename" => {
                            // Must be an array of "RustVariant=SmithyMember" strings.
                            match &value {
                                Expr::Array(ExprArray { elems, .. }) => {
                                    for elem in elems {
                                        let s = expr_to_string(elem)?;
                                        let (rust_name, smithy_name) =
                                            s.split_once('=').ok_or_else(|| {
                                                syn::Error::new_spanned(
                                                    elem,
                                                    "rename entry must look like \"RustVariant=SmithyMember\"",
                                                )
                                            })?;
                                        args.rename.insert(
                                            rust_name.trim().to_string(),
                                            smithy_name.trim().to_string(),
                                        );
                                    }
                                }
                                _ => {
                                    return Err(syn::Error::new_spanned(
                                        &value,
                                        "rename must be a list: rename = [\"RustVariant=SmithyMember\", ...]",
                                    ));
                                }
                            }
                        }
                        other => {
                            return Err(syn::Error::new_spanned(
                                &path,
                                format!("unknown smithy attribute key: `{other}`"),
                            ));
                        }
                    }
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        other,
                        "expected `key = value` syntax",
                    ));
                }
            }
        }

        Ok(args)
    }

    fn shape_key(&self) -> Option<&str> {
        if self.input.is_some() {
            Some("input")
        } else if self.output.is_some() {
            Some("output")
        } else if self.structure.is_some() {
            Some("structure")
        } else if self.enum_for.is_some() {
            Some("enum_for")
        } else if self.union_for.is_some() {
            Some("union_for")
        } else if self.error.is_some() {
            Some("error")
        } else if self.error_for.is_some() {
            Some("error_for")
        } else if self.shape.is_some() {
            Some("shape")
        } else {
            None
        }
    }

    fn shape_id(&self) -> Option<&str> {
        self.input
            .as_deref()
            .or(self.output.as_deref())
            .or(self.structure.as_deref())
            .or(self.enum_for.as_deref())
            .or(self.union_for.as_deref())
            .or(self.error.as_deref())
            .or(self.error_for.as_deref())
            .or(self.shape.as_deref())
    }

    /// The Smithy model path for this invocation. Required: must be set
    /// either via `model = "..."` on the attribute itself, or (typically)
    /// injected by a `smithy_model!("...")` declaration earlier in the
    /// module. Returns an error if neither is present.
    fn model_path(&self) -> syn::Result<&str> {
        self.model.as_deref().ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                "no Smithy model path set: add `smithy_model!(\"path/to/model.json\")` at the top of the module, or pass `model = \"...\"` to this attribute",
            )
        })
    }
}

fn expr_to_string(expr: &syn::Expr) -> syn::Result<String> {
    if let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(s),
        ..
    }) = expr
    {
        Ok(s.value())
    } else {
        Err(syn::Error::new_spanned(expr, "expected a string literal"))
    }
}

// =============================================================================
// Struct handling (input/output/structure/error)
// =============================================================================

fn smithy_on_struct(args: SmithyArgs, item: &syn::ItemStruct) -> syn::Result<TokenStream> {
    let key = args.shape_key().ok_or_else(|| {
        syn::Error::new_spanned(
            &item.ident,
            "#[smithy(...)] on a struct requires one of: input, output, structure, or error",
        )
    })?;

    match key {
        "enum_for" => {
            return Err(syn::Error::new_spanned(
                &item.ident,
                "`enum_for` can only be used on an enum, not a struct",
            ));
        }
        "union_for" => {
            return Err(syn::Error::new_spanned(
                &item.ident,
                "`union_for` can only be used on an enum, not a struct",
            ));
        }
        "error_for" => {
            return Err(syn::Error::new_spanned(
                &item.ident,
                "`error_for` can only be used on an enum, not a struct",
            ));
        }
        "error" => return smithy_error_variant_struct(args, item),
        _ => {} // input/output/structure -> proceed with struct codegen
    }

    // Short-circuit for empty (Unit) structs: skip model validation entirely.
    let has_no_fields = match &item.fields {
        syn::Fields::Named(f) => f.named.is_empty(),
        syn::Fields::Unit => true,
        _ => false,
    };
    if has_no_fields {
        let shape_id_str = args.shape_id().unwrap().to_string();
        return emit_empty_struct(item, &shape_id_str);
    }

    let shape_id_owned = args.shape_id().unwrap().to_string();
    let model_path = args.model_path()?.to_string();
    let unsupported_fields: HashSet<String> = args.unsupported.into_iter().collect();
    let skip_validation_fields: HashSet<String> = args.skip_validation.into_iter().collect();
    let name = &item.ident;

    let model = load_smithy_model(&model_path).map_err(|e| {
        syn::Error::new_spanned(name, format!("Failed to load Smithy model: {}", e))
    })?;

    let shape = model.get(&shape_id_owned).ok_or_else(|| {
        syn::Error::new_spanned(
            name,
            shape_not_found_message("Shape", &shape_id_owned, &model),
        )
    })?;

    let derive_input: DeriveInput = syn::parse_quote! { #item };
    validate_struct_against_shape(&derive_input, shape, &model, &unsupported_fields)?;

    let impls = emit_struct_impls(
        &derive_input,
        &shape_id_owned,
        shape,
        &model,
        &unsupported_fields,
        &skip_validation_fields,
    )?;

    // Re-emit the original item unchanged, plus generated impls
    Ok(quote! {
        #item
        #impls
    }
    .into())
}

fn smithy_error_variant_struct(
    args: SmithyArgs,
    item: &syn::ItemStruct,
) -> syn::Result<TokenStream> {
    let shape_id_owned = args.shape_id().unwrap().to_string();
    let model_path = args.model_path()?.to_string();
    let unsupported_fields: HashSet<String> = args.unsupported.into_iter().collect();
    let name = &item.ident;

    let model = load_smithy_model(&model_path).map_err(|e| {
        syn::Error::new_spanned(name, format!("Failed to load Smithy model: {}", e))
    })?;

    let shape = model.get(&shape_id_owned).ok_or_else(|| {
        syn::Error::new_spanned(
            name,
            shape_not_found_message("Error shape", &shape_id_owned, &model),
        )
    })?;

    // Verify it has the @error trait
    let has_error_trait = shape
        .get("traits")
        .and_then(|t| t.as_object())
        .map(|t| t.contains_key("smithy.api#error"))
        .unwrap_or(false);

    if !has_error_trait {
        return Err(syn::Error::new_spanned(
            name,
            format!(
                "Shape '{}' is not an error (missing @error trait)",
                shape_id_owned
            ),
        ));
    }

    let derive_input: DeriveInput = syn::parse_quote! { #item };
    validate_struct_against_shape(&derive_input, shape, &model, &unsupported_fields)?;

    // Extract the local shape name
    let local_name = shape_id_owned.split('#').nth(1).unwrap_or(&shape_id_owned);
    let namespace = shape_id_owned.split('#').next().unwrap_or("smithy.unknown");

    // Generate the SCHEMA_* constant name that error_for enums reference
    let schema_const_name = error_name_to_schema_const(&name.to_string());
    let schema_ident = syn::Ident::new(&schema_const_name, name.span());

    let members = shape.get("members").and_then(|m| m.as_object());

    let Data::Struct(data_struct) = &derive_input.data else {
        return Err(syn::Error::new_spanned(name, "expected a struct"));
    };
    let Fields::Named(named) = &data_struct.fields else {
        return Err(syn::Error::new_spanned(name, "expected named fields"));
    };

    // Build module-level member statics and serialize arms
    let mut module_statics = Vec::new();
    let mut module_static_refs = Vec::new();
    let mut serialize_arms = Vec::new();

    if let Some(members) = members {
        let mut emitted_index: usize = 0;
        for (smithy_name, smithy_member) in members.iter() {
            if unsupported_fields.contains(smithy_name) {
                continue;
            }
            let rust_name = to_snake_case(smithy_name);
            // `make_field_ident`, so a member whose snake_case name is a Rust keyword
            // (Lambda/SNS `Type` -> `type`) is emitted as the RAW ident `r#type`.
            // `format_ident!` does not panic here — `Ident::new` accepts keywords — it
            // just yields the bare token `type`, which is a syntax error where the field
            // is USED. Both halves are needed: this produces the ident, and the lookup
            // below has to strip the `r#` back off to find the declared field.
            let field_ident = make_field_ident(&rust_name);
            let static_name = quote::format_ident!(
                "{}_M_{}",
                to_snake_case(&name.to_string()).to_uppercase(),
                to_snake_case(smithy_name).to_uppercase()
            );
            let idx_lit =
                syn::LitInt::new(&emitted_index.to_string(), proc_macro2::Span::call_site());

            let kind = codegen::classify_member(smithy_member, &model).ok_or_else(|| {
                syn::Error::new_spanned(
                    name,
                    format!("internal: cannot classify Smithy member '{smithy_name}'"),
                )
            })?;
            let shape_type = codegen::shape_type_ident(&kind);

            // XML element name for this error member. Both XML protocols
            // (`restXml` and `awsQuery`/`ec2Query`) render error members through
            // the XML serializer, which reads `Schema::xml_name()` and otherwise
            // falls back to the raw Smithy member name. Smithy names the error
            // detail member `message` (lowercase), but every AWS service puts
            // `<Message>` on the wire and every SDK error parser looks for
            // `Error/Message` — botocore substitutes the string "Unknown" when
            // it is missing. Emitting `<message>` therefore silently discards
            // the error text for XML-protocol services. An explicit model
            // `@xmlName` always wins; `message` defaults to `Message`.
            let member_xml_name = smithy_member
                .get("traits")
                .and_then(|t| t.get("smithy.api#xmlName"))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
                .or_else(|| (smithy_name == "message").then(|| "Message".to_owned()));
            let xml_name_chain = match &member_xml_name {
                Some(xn) => quote! { .with_xml_name(#xn) },
                None => quote! {},
            };

            // JSON object key for this error member. The restJson1/awsJson error
            // serializers read `Schema::json_name()` and otherwise fall back to
            // the member name, so an explicit model `@jsonName` renames the key
            // on the wire. No `message` fallback here: SDK JSON error parsers
            // read the lowercase `message` member directly, which is the Smithy
            // default, so only an explicit `@jsonName` changes the key.
            let json_name_chain =
                json_name_chain(smithy_member.get("traits").and_then(|t| t.as_object()));

            module_statics.push(quote! {
                static #static_name: ::aws_smithy_schema::Schema =
                    ::aws_smithy_schema::Schema::new_member(
                        ::aws_smithy_schema::ShapeId::from_static(
                            concat!(#namespace, "#", #local_name, "$", #smithy_name),
                            #namespace,
                            #smithy_name,
                        ),
                        #shape_type,
                        #smithy_name,
                        #idx_lit,
                    )#xml_name_chain #json_name_chain;
            });
            module_static_refs.push(quote! { &#static_name });

            // Strip a leading `r#` before matching, exactly as `emit_struct_impls` and
            // `validate_struct_against_shape` already do. A member whose snake_case name
            // is a Rust keyword (`Type` -> `type`) must be declared `pub r#type`, and
            // `syn` stringifies that ident as "r#type", so an unstripped comparison
            // against `rust_name` ("type") misses the field that is actually there.
            let rust_field = named.named.iter().find(|f| {
                f.ident.as_ref().map(|i| {
                    let s = i.to_string();
                    s.strip_prefix("r#").unwrap_or(&s).to_string()
                }) == Some(rust_name.clone())
            });
            // A miss here is a DIAGNOSTIC, never a panic. `validate_struct_against_shape`
            // is expected to have refused this struct already, but the two must agree on
            // how a member name maps to a field ident — and if they ever diverge, a panic
            // in a proc macro aborts the whole build with no source location. A spanned
            // `syn::Error` points at the offending struct and names both sides instead.
            let field_ty = match rust_field.map(|f| &f.ty) {
                Some(ty) => ty,
                None => {
                    let declared: Vec<String> = named
                        .named
                        .iter()
                        .filter_map(|f| f.ident.as_ref().map(|i| i.to_string()))
                        .collect();
                    return Err(syn::Error::new_spanned(
                        name,
                        format!(
                            "member `{smithy_name}` of `{shape_id_owned}` maps to field \
                             `{rust_name}`, which this struct does not declare (it declares: \
                             {}). Add the field, or list the member in \
                             `unsupported = [..]`.",
                            declared.join(", ")
                        ),
                    ));
                }
            };
            let is_option = codegen::unwrap_option(field_ty).is_some();
            let inner_ty: &syn::Type = codegen::unwrap_option(field_ty).unwrap_or(field_ty);

            let member_expr = quote! { &#static_name };

            let write_value_expr = if is_option {
                quote! { __v }
            } else {
                quote! { (&self.#field_ident) }
            };
            let write_stmt = codegen::emit_write(
                &kind,
                inner_ty,
                &member_expr,
                &write_value_expr,
                &quote! { __s },
            )?;

            if is_option {
                serialize_arms.push(quote! {
                    if let Some(__v) = self.#field_ident.as_ref() {
                        #write_stmt
                    }
                });
            } else {
                serialize_arms.push(write_stmt);
            }

            emitted_index += 1;
        }
    }

    // The `: <message>` suffix on the generated `Display`. A Smithy error's
    // `message` member may be optional OR carry `@required`/`@default`, and the
    // two produce DIFFERENT Rust field types — `Option<String>` versus a bare
    // `String` — so the pattern used to read it must follow the field, not be
    // assumed. Matching on the declared type keeps the emitted `Display` valid
    // for both; assuming `Option` makes every `@required`-message error shape
    // fail to compile with `E0308`.
    let message_field = named
        .named
        .iter()
        .find(|f| f.ident.as_ref().map(|i| i.to_string()) == Some("message".to_string()));
    let display_message_arm = match message_field {
        Some(f) if codegen::unwrap_option(&f.ty).is_some() => quote! {
            if let Some(msg) = &self.message {
                write!(f, ": {}", msg)?;
            }
        },
        Some(_) => quote! {
            write!(f, ": {}", &self.message)?;
        },
        None => quote! {},
    };

    Ok(quote! {
        #item

        #(#module_statics)*

        pub static #schema_ident: ::aws_smithy_schema::Schema =
            ::aws_smithy_schema::Schema::new_struct(
                ::aws_smithy_schema::ShapeId::from_static(
                    #shape_id_owned,
                    #namespace,
                    #local_name,
                ),
                ::aws_smithy_schema::ShapeType::Structure,
                &[#(#module_static_refs,)*],
            );

        impl #name {
            pub const fn shape_name() -> &'static str {
                #local_name
            }
        }

        impl ::std::fmt::Display for #name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                write!(f, "{}", #local_name)?;
                #display_message_arm
                Ok(())
            }
        }

        impl ::std::error::Error for #name {}

        impl ::aws_smithy_schema::serde::SerializableStruct for #name {
            fn serialize_members(
                &self,
                __s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
            ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
                #(#serialize_arms)*
                Ok(())
            }
        }
    }
    .into())
}

// =============================================================================
// Enum handling (enum_for / error_for)
// =============================================================================

fn smithy_on_enum(args: SmithyArgs, item: &syn::ItemEnum) -> syn::Result<TokenStream> {
    let key = args.shape_key().ok_or_else(|| {
        syn::Error::new_spanned(
            &item.ident,
            "#[smithy(...)] on an enum requires one of: enum_for, union_for, or error_for",
        )
    })?;

    match key {
        "input" | "output" | "structure" => {
            Err(syn::Error::new_spanned(
                &item.ident,
                format!("`{key}` can only be used on a struct, not an enum"),
            ))
        }
        "error" => {
            Err(syn::Error::new_spanned(
                &item.ident,
                "`error` can only be used on a struct (error variant), not an enum. Use `error_for` for operation error enums.",
            ))
        }
        "enum_for" => smithy_enum_impl(args, item),
        "union_for" => smithy_union_impl(args, item),
        "error_for" => smithy_error_enum_impl(args, item),
        _ => unreachable!(),
    }
}

fn smithy_enum_impl(args: SmithyArgs, item: &syn::ItemEnum) -> syn::Result<TokenStream> {
    let shape_id_owned = args.shape_id().unwrap().to_string();
    let model_path = args.model_path()?.to_string();
    let name = &item.ident;

    let model = load_smithy_model(&model_path).map_err(|e| {
        syn::Error::new_spanned(name, format!("Failed to load Smithy model: {}", e))
    })?;

    let shape = model.get(&shape_id_owned).ok_or_else(|| {
        syn::Error::new_spanned(
            name,
            shape_not_found_message("Shape", &shape_id_owned, &model),
        )
    })?;

    let shape_type = shape.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if shape_type != "enum" {
        return Err(syn::Error::new_spanned(
            name,
            format!(
                "Shape '{}' is type '{}', not 'enum'. Use `input`/`output`/`structure` for structure shapes.",
                shape_id_owned, shape_type
            ),
        ));
    }

    // Get enum members from the model
    let smithy_members = shape
        .get("members")
        .and_then(|m| m.as_object())
        .ok_or_else(|| syn::Error::new_spanned(name, "Enum shape has no members"))?;

    // Validate all variants are unit-style and match the model
    let mut rust_variants: HashMap<String, &syn::Variant> = HashMap::new();
    for variant in &item.variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(syn::Error::new_spanned(
                variant,
                "Smithy enum variants must be unit-style (no fields)",
            ));
        }
        rust_variants.insert(variant.ident.to_string(), variant);
    }

    // Map Rust PascalCase -> Smithy SCREAMING_SNAKE_CASE
    let mut from_arms = Vec::new();
    let mut into_arms = Vec::new();

    for (smithy_name, member_value) in smithy_members.iter() {
        let rust_name = screaming_snake_to_pascal(smithy_name);
        if !rust_variants.contains_key(&rust_name) {
            return Err(syn::Error::new_spanned(
                name,
                format!(
                    "Smithy enum '{}' declares member '{}' (Rust: `{}`), but the enum doesn't have that variant. Add:\n    {},",
                    shape_id_owned, smithy_name, rust_name, rust_name
                ),
            ));
        }
        rust_variants.remove(&rust_name);

        // Use @enumValue trait if present, otherwise fall back to member name
        let wire_value = member_value
            .get("traits")
            .and_then(|t| t.get("smithy.api#enumValue"))
            .and_then(|v| v.as_str())
            .unwrap_or(smithy_name);

        // PascalCase, so only `Self` can collide — still routed through the
        // keyword-safe helper rather than relying on that staying true.
        let variant_ident = make_field_ident(&rust_name);
        from_arms.push(quote! { #wire_value => Ok(#name::#variant_ident), });
        into_arms.push(quote! { #name::#variant_ident => #wire_value, });
    }

    // Check for extra variants
    if let Some((extra, variant)) = rust_variants.iter().next() {
        return Err(syn::Error::new_spanned(
            variant,
            format!(
                "Variant '{}' does not correspond to any Smithy enum member in '{}'",
                extra, shape_id_owned
            ),
        ));
    }

    let error_type = quote::format_ident!("Parse{}Error", name);

    Ok(quote! {
        #item

        #[derive(Debug)]
        pub struct #error_type(String);

        impl std::fmt::Display for #error_type {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "unknown {}: {}", stringify!(#name), self.0)
            }
        }

        impl std::error::Error for #error_type {}

        impl ::smithy_server_runtime::wire::FromSmithyString for #name {
            type Err = #error_type;
            fn from_smithy_string(s: &str) -> Result<Self, Self::Err> {
                match s {
                    #(#from_arms)*
                    other => Err(#error_type(other.to_string())),
                }
            }
        }

        impl #name {
            /// Return the Smithy wire-format string for this variant.
            pub fn as_str(&self) -> &str {
                match self {
                    #(#into_arms)*
                }
            }
        }
    }
    .into())
}

/// Emit Smithy union codegen.
///
/// On the wire, awsJson1.0/1.1 encodes a union as a single-key object —
/// `{"S":"hello"}`, `{"L":[…]}`, etc. Structurally that's identical to a
/// struct where exactly one optional member is populated, so we lean on the
/// existing `SerializableStruct` / `DeserializableStruct` runtime traits and
/// the existing JSON codec instead of introducing new traits or codec
/// methods.
///
/// The macro generates:
/// - a per-variant `M_<NAME>` member-schema static, classified from the
///   Smithy member's target shape;
/// - a top-level `SCHEMA` constant with `ShapeType::Union`;
/// - `SerializableStruct::serialize_members` that matches `self` and writes
///   exactly the active variant;
/// - `DeserializableStruct::deserialize` that runs `read_struct`, dispatches
///   on `member_index()`, and constructs the corresponding variant.
///
/// Variant naming: by default, Smithy member name → Rust variant via
/// `screaming_snake_to_pascal` (mirrors `enum_for`). Override with
/// `rename = ["RustVariant=SmithyMember", …]` for engine types that don't
/// follow that convention (e.g. DDB's `StringSet` ↔ `SS`).
fn smithy_union_impl(args: SmithyArgs, item: &syn::ItemEnum) -> syn::Result<TokenStream> {
    let shape_id_owned = args.shape_id().unwrap().to_string();
    let model_path = args.model_path()?.to_string();
    let name = &item.ident;
    let renames = args.rename;
    // Union members the emulator doesn't model (e.g. an event stream's exception
    // events): skipped from validation + codegen, like struct `unsupported`.
    let unsupported: HashSet<String> = args.unsupported.into_iter().collect();

    let model = load_smithy_model(&model_path).map_err(|e| {
        syn::Error::new_spanned(name, format!("Failed to load Smithy model: {}", e))
    })?;

    let shape = model.get(&shape_id_owned).ok_or_else(|| {
        syn::Error::new_spanned(
            name,
            shape_not_found_message("Shape", &shape_id_owned, &model),
        )
    })?;

    let shape_type = shape.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if shape_type != "union" {
        return Err(syn::Error::new_spanned(
            name,
            format!(
                "Shape '{}' is type '{}', not 'union'. Use `enum_for` for plain enums or `input`/`output`/`structure` for structures.",
                shape_id_owned, shape_type
            ),
        ));
    }

    let smithy_members = shape
        .get("members")
        .and_then(|m| m.as_object())
        .ok_or_else(|| syn::Error::new_spanned(name, "Union shape has no members"))?;

    // Validate every Rust variant is tuple-style with exactly one field.
    let mut rust_variants: HashMap<String, &syn::Variant> = HashMap::new();
    for variant in &item.variants {
        match &variant.fields {
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {}
            _ => {
                return Err(syn::Error::new_spanned(
                    variant,
                    "Smithy union variants must be tuple-style with exactly one field, like `S(String)`",
                ));
            }
        }
        rust_variants.insert(variant.ident.to_string(), variant);
    }

    // Build Rust→Smithy variant map. Default mapping for each Smithy member
    // is `screaming_snake_to_pascal`. Renames override that.
    let mut smithy_to_rust: HashMap<String, String> = HashMap::new();
    let rust_to_smithy_renames: HashMap<&String, &String> = renames.iter().collect();
    let smithy_to_rust_renames: HashMap<&String, &String> =
        renames.iter().map(|(r, s)| (s, r)).collect();

    for smithy_name in smithy_members.keys() {
        if unsupported.contains(smithy_name) {
            continue;
        }
        let rust_name = if let Some(rust) = smithy_to_rust_renames.get(smithy_name) {
            (*rust).clone()
        } else {
            screaming_snake_to_pascal(smithy_name)
        };
        if !rust_variants.contains_key(&rust_name) {
            return Err(syn::Error::new_spanned(
                name,
                format!(
                    "Smithy union '{}' declares member '{}' (Rust: `{}`), but the enum doesn't have that variant. Add `{}({})` or rename via `rename = [\"...\"]`.",
                    shape_id_owned,
                    smithy_name,
                    rust_name,
                    rust_name,
                    "T",
                ),
            ));
        }
        smithy_to_rust.insert(smithy_name.clone(), rust_name);
    }

    // Check every Rust variant maps to a Smithy member.
    for (rust_name, variant) in &rust_variants {
        let mapped = if let Some(smithy) = rust_to_smithy_renames.get(rust_name) {
            (*smithy).clone()
        } else {
            // Inverse-lookup: did any Smithy member map to this Rust name?
            smithy_to_rust
                .iter()
                .find(|(_, r)| *r == rust_name)
                .map(|(s, _)| s.clone())
                .unwrap_or_default()
        };
        if mapped.is_empty() || !smithy_members.contains_key(&mapped) {
            return Err(syn::Error::new_spanned(
                variant,
                format!(
                    "Variant '{}' does not correspond to any Smithy union member in '{}'. Add a `rename = [\"{}=…\"]` entry if the Smithy member name differs.",
                    rust_name, shape_id_owned, rust_name
                ),
            ));
        }
    }

    let namespace = shape_id_owned.split('#').next().unwrap_or("smithy.unknown");
    let local_name = shape_id_owned.split('#').nth(1).unwrap_or("Unknown");

    let is_event_stream = codegen::is_event_stream_union(&shape_id_owned, &model);

    // Walk Smithy members in declaration order so member_index() matches the
    // order baked into the SCHEMA constant's member list. Unsupported members
    // are skipped, so the schema index is compacted (matches struct codegen).
    let mut member_static_decls: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut member_static_refs: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut serialize_arms: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut deserialize_arms: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut event_stream_arms: Vec<proc_macro2::TokenStream> = Vec::new();

    let mut emitted_index: usize = 0;
    for (smithy_name, smithy_member) in smithy_members.iter() {
        if unsupported.contains(smithy_name) {
            continue;
        }
        let idx = emitted_index;
        emitted_index += 1;
        let rust_name = smithy_to_rust.get(smithy_name).cloned().unwrap();
        let variant = rust_variants
            .get(&rust_name)
            .copied()
            .expect("validated above");
        let variant_ident = &variant.ident;

        // Single tuple-field type — validated above.
        let Fields::Unnamed(fields) = &variant.fields else {
            unreachable!("validated tuple-style variant");
        };
        let field_ty = &fields.unnamed.first().unwrap().ty;

        let kind = codegen::classify_member(smithy_member, &model).ok_or_else(|| {
            syn::Error::new_spanned(
                variant,
                format!("internal: cannot classify Smithy union member '{smithy_name}'"),
            )
        })?;
        let shape_type_tok = codegen::shape_type_ident(&kind);

        let static_name = quote::format_ident!("M_{}", to_snake_case(smithy_name).to_uppercase());
        let idx_lit = syn::LitInt::new(&idx.to_string(), proc_macro2::Span::call_site());

        member_static_decls.push(quote! {
            const #static_name: ::aws_smithy_schema::Schema =
                ::aws_smithy_schema::Schema::new_member(
                    ::aws_smithy_schema::ShapeId::from_static(
                        concat!(#namespace, "#", #local_name, "$", #smithy_name),
                        #namespace,
                        #smithy_name,
                    ),
                    #shape_type_tok,
                    #smithy_name,
                    #idx_lit,
                );
        });
        member_static_refs.push(quote! { &Self::#static_name });

        let member_expr = quote! { &Self::#static_name };

        // Serialize: variant pattern → write the inner value via codegen.
        let write_value_expr = quote! { __v };
        let write_stmt = codegen::emit_write(
            &kind,
            field_ty,
            &member_expr,
            &write_value_expr,
            &quote! { __s },
        )?;
        serialize_arms.push(quote! {
            Self::#variant_ident(__v) => {
                #write_stmt
            }
        });

        // Deserialize: member_index() == idx → read the inner value and
        // construct the variant.
        let read_expr = codegen::emit_read(&kind, field_ty, &member_expr, &quote! { __d })?;
        let arm_idx = syn::LitInt::new(&idx.to_string(), proc_macro2::Span::call_site());
        deserialize_arms.push(quote! {
            Some(#arm_idx) => {
                // A union carries EXACTLY one member, so a second member on the
                // wire is ambiguous and must be refused. Overwriting instead
                // (last-member-wins) is worse than it looks: the request parses,
                // the handler receives a well-formed value, and the response is
                // 200 — for a value the caller did not send. By the time the
                // handler runs the duplicate is unrecoverable, so this is the
                // only place the arity can be enforced.
                if __out.is_some() {
                    return ::std::result::Result::Err(
                        ::aws_smithy_schema::serde::SerdeError::custom(
                            format!("union '{}' had more than one member set", #local_name),
                        ),
                    );
                }
                __out = Some(Self::#variant_ident(#read_expr));
            }
        });

        // Event-stream arm: each variant frames its inner struct via the
        // protocol. An `@error` member becomes an in-band exception event.
        if is_event_stream {
            let target = smithy_member
                .get("target")
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let is_exception = model
                .get(target)
                .and_then(|s| s.get("traits"))
                .and_then(|t| t.as_object())
                .is_some_and(|t| t.contains_key("smithy.api#error"));
            event_stream_arms.push(quote! {
                Self::#variant_ident(__v) => __protocol.serialize_event_stream_message(
                    #smithy_name,
                    #is_exception,
                    __v,
                    &#field_ty::SCHEMA,
                ),
            });
        }
    }

    let struct_id_lit = shape_id_owned.clone();
    let schema_const_name =
        quote::format_ident!("{}_SCHEMA", to_snake_case(&name.to_string()).to_uppercase());

    // For a `@streaming` union, emit a method that frames the active variant as
    // one event-stream message via the protocol. The service dispatch calls
    // this instead of `serialize_output` for a streaming-output operation.
    let event_stream_method = if is_event_stream {
        quote! {
            pub fn serialize_as_event_stream<P>(
                &self,
                __protocol: &P,
            ) -> ::std::result::Result<P::Response, ::aws_smithy_schema_server::ProtocolError>
            where
                P: ::aws_smithy_schema_server::ServerProtocol,
            {
                match self {
                    #(#event_stream_arms)*
                }
            }
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        #item

        #[allow(non_upper_case_globals)]
        impl #name {
            #(#member_static_decls)*

            pub const SCHEMA: ::aws_smithy_schema::Schema =
                ::aws_smithy_schema::Schema::new_struct(
                    ::aws_smithy_schema::ShapeId::from_static(
                        #struct_id_lit,
                        #namespace,
                        #local_name,
                    ),
                    ::aws_smithy_schema::ShapeType::Union,
                    &[#(#member_static_refs,)*],
                );

            #event_stream_method
        }

        pub static #schema_const_name: &::aws_smithy_schema::Schema = &#name::SCHEMA;

        impl ::aws_smithy_schema::serde::SerializableStruct for #name {
            fn serialize_members(
                &self,
                __s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
            ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
                match self {
                    #(#serialize_arms)*
                }
                Ok(())
            }
        }

        impl ::aws_smithy_schema_server::DeserializableStruct for #name {
            fn deserialize(
                __d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
            ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
                let mut __out: Option<Self> = None;
                __d.read_struct(&#name::SCHEMA, &mut |__member, __d| {
                    match __member.member_index() {
                        #(#deserialize_arms)*
                        _ => {}
                    }
                    Ok(())
                })?;
                __out.ok_or_else(|| {
                    ::aws_smithy_schema::serde::SerdeError::custom(
                        format!("union '{}' had no member set", #local_name),
                    )
                })
            }
        }
    }
    .into())
}

fn smithy_error_enum_impl(args: SmithyArgs, item: &syn::ItemEnum) -> syn::Result<TokenStream> {
    let operation_id = args.shape_id().unwrap().to_string();
    let model_path = args.model_path()?.to_string();
    let name = &item.ident;

    let model = load_smithy_model(&model_path).map_err(|e| {
        syn::Error::new_spanned(name, format!("Failed to load Smithy model: {}", e))
    })?;

    let operation_shape = model.get(&operation_id).ok_or_else(|| {
        syn::Error::new_spanned(
            name,
            shape_not_found_message("Operation", &operation_id, &model),
        )
    })?;

    // An operation with no `errors` list has no modeled errors: tolerate a
    // missing/absent `errors` array by treating it as empty rather than
    // failing codegen. Operations that DO declare errors are unaffected.
    const NO_ERRORS: &[serde_json::Value] = &[];
    let smithy_errors: &[serde_json::Value] = operation_shape
        .get("errors")
        .and_then(|e| e.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(NO_ERRORS);

    let mut expected_errors: HashSet<String> = HashSet::new();
    for error in smithy_errors {
        let target = error
            .get("target")
            .and_then(|t| t.as_str())
            .ok_or_else(|| syn::Error::new_spanned(name, "Error missing target"))?;
        let local_name = target
            .split('#')
            .nth(1)
            .ok_or_else(|| syn::Error::new_spanned(name, "Invalid error shape ID"))?;
        expected_errors.insert(local_name.to_string());
    }

    // Add extra_errors to the expected set so they pass validation
    let extra_errors: HashSet<String> = args.extra_errors.into_iter().collect();
    for extra in &extra_errors {
        expected_errors.insert(extra.clone());
    }

    let mut actual_variants: HashSet<String> = HashSet::new();
    for variant in &item.variants {
        let variant_name = variant.ident.to_string();
        actual_variants.insert(variant_name.clone());
        match &variant.fields {
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {}
            _ => {
                return Err(syn::Error::new_spanned(
                    variant,
                    format!(
                        "Variant '{}' must be a tuple variant with exactly one field, like: {}({})",
                        variant_name, variant_name, variant_name
                    ),
                ));
            }
        }
    }

    for expected in &expected_errors {
        if !actual_variants.contains(expected) {
            let op_name = operation_id.split('#').nth(1).unwrap_or(&operation_id);
            return Err(syn::Error::new_spanned(
                name,
                format!(
                    "Operation '{}' declares error '{}', but '{}' doesn't have that variant. Add:\n    {}({}),",
                    op_name, expected, name, expected, expected
                ),
            ));
        }
    }

    for actual in &actual_variants {
        if !expected_errors.contains(actual) {
            let op_name = operation_id.split('#').nth(1).unwrap_or(&operation_id);
            return Err(syn::Error::new_spanned(
                name,
                format!(
                    "'{}::{}' is not in '{}'s declared errors. Add `extra_errors = [\"{}\"]` if this is intentional.",
                    name, actual, op_name, actual
                ),
            ));
        }
    }

    // Generate match arms for each variant
    let variant_idents: Vec<&syn::Ident> = item.variants.iter().map(|v| &v.ident).collect();

    let display_arms = variant_idents.iter().map(|v| {
        quote! { Self::#v(e) => write!(f, "{}", e), }
    });

    let as_serializable_arms = variant_idents.iter().map(|v| {
        quote! { Self::#v(e) => e, }
    });

    let schema_idents: Vec<syn::Ident> = variant_idents
        .iter()
        .map(|v| {
            let schema_name = error_name_to_schema_const(&v.to_string());
            syn::Ident::new(&schema_name, v.span())
        })
        .collect();

    let variant_schema_arms = variant_idents
        .iter()
        .zip(schema_idents.iter())
        .map(|(v, s)| {
            quote! { Self::#v(_) => &#s, }
        });

    let shape_name_arms = variant_idents.iter().map(|v| {
        quote! { Self::#v(_) => #v::shape_name(), }
    });

    let namespace = operation_id.split('#').next().unwrap_or("");

    let query_error_code_arms = variant_idents.iter().map(|v| {
        let variant_name = v.to_string();
        let shape_id = format!("{}#{}", namespace, variant_name);
        let code = model
            .get(&shape_id)
            .and_then(|s| s.get("traits"))
            .and_then(|t| t.as_object())
            .and_then(|t| t.get("aws.protocols#awsQueryError"))
            .and_then(|v| v.get("code"))
            .and_then(|c| c.as_str());
        match code {
            Some(c) => quote! { Self::#v(_) => Some(#c), },
            None => quote! { Self::#v(_) => None, },
        }
    });

    // Determine HTTP status for each variant from its modeled traits, the same
    // way for every variant — a variant widened in through `extra_errors` is no
    // exception. Prefer the explicit `@httpError` trait, which pins an exact
    // status code (403, 429, 404, ...). Fall back to the `@error` trait
    // (server => 500, client => 400). Fall back to 400 only when the error is
    // unmodeled: a shape the model does not declare, or one carrying neither
    // trait, has no status to read, so a client-error default is the safe choice.
    let http_status_arms = variant_idents.iter().map(|v| {
        let variant_name = v.to_string();
        let shape_id = format!("{}#{}", namespace, variant_name);
        let traits = model
            .get(&shape_id)
            .and_then(|s| s.get("traits"))
            .and_then(|t| t.as_object());
        let status: u16 = traits
            .and_then(|t| t.get("smithy.api#httpError"))
            .and_then(|c| c.as_u64())
            .map(|c| c as u16)
            .or_else(|| {
                traits
                    .and_then(|t| t.get("smithy.api#error"))
                    .and_then(|e| e.as_str())
                    .map(|e| if e == "server" { 500 } else { 400 })
            })
            .unwrap_or(400);
        quote! { Self::#v(_) => #status, }
    });

    // An error enum with no variants (an operation that models no errors) is
    // uninhabited. `match self { }` does not type-check because a reference is
    // always considered inhabited, so match on the dereferenced place instead:
    // `match *self { }` is exhaustive for an uninhabited type.
    let scrutinee = if variant_idents.is_empty() {
        quote! { *self }
    } else {
        quote! { self }
    };

    Ok(quote! {
        #item

        impl std::fmt::Display for #name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match #scrutinee {
                    #(#display_arms)*
                }
            }
        }

        impl std::error::Error for #name {}

        impl ::smithy_server_runtime::OperationError for #name {
            fn as_serializable(&self) -> &dyn ::aws_smithy_schema::serde::SerializableStruct {
                match #scrutinee {
                    #(#as_serializable_arms)*
                }
            }
            fn variant_schema(&self) -> &'static ::aws_smithy_schema::Schema {
                match #scrutinee {
                    #(#variant_schema_arms)*
                }
            }
            fn shape_name(&self) -> &'static str {
                match #scrutinee {
                    #(#shape_name_arms)*
                }
            }
            fn http_status(&self) -> u16 {
                match #scrutinee {
                    #(#http_status_arms)*
                }
            }
            fn query_error_code(&self) -> Option<&'static str> {
                match #scrutinee {
                    #(#query_error_code_arms)*
                }
            }
        }
    }
    .into())
}

/// Render the expression a `@default` (but not `@required`) member falls back to
/// when the member is absent from the wire.
///
/// Such a member is a bare, non-`Option` field, so the deserializer substitutes
/// the model's declared default. `Timestamp` materializes that value explicitly
/// and never relies on `Default`, because `aws_smithy_types::DateTime` has no
/// `Default` impl and the orphan rule forbids adding one.
///
/// Smithy encodes a timestamp default as epoch seconds. A whole number maps to
/// `DateTime::from_secs` and a fractional one to `DateTime::from_secs_f64`; any
/// other form (or an absent value) falls back to epoch `0`, the Smithy zero
/// value for a timestamp. A malformed default therefore still yields a valid
/// crate rather than failing codegen for the whole service.
///
/// A `string` and an `enum` shape both materialize the declared value, since an
/// enum shape is generated as a `String` field. Every other shape kind uses
/// `Default::default()` and ignores `default_value`. Two consequences worth
/// knowing:
///
/// * `Default` yields the *type's* zero value, so a member declaring a non-zero
///   default (an `integer` `@default: 100`, say) deserializes to `0` when
///   absent. Materializing those faithfully needs the field type rather than the
///   shape kind: a numeric literal must agree with the field's width, and an
///   `enum` member's Rust type may be a user-authored enum reached through
///   `FromSmithyString` rather than a `String`.
/// * For the same reason, `Default` is not *guaranteed* here — it is only known
///   to hold for the `aws_smithy_types` scalars (`Blob` derives it, `Document`
///   implements it). A `@default` member backed by a user-authored `enum_for`
///   type that does not derive `Default` would reproduce the same missing-bound
///   error this function avoids for `DateTime`; nothing in this macro checks for
///   that.
/// The member's DECLARED `@default`, or `None` when it has no usable one.
///
/// Three call sites asked `traits.contains_key("smithy.api#default")` independently.
/// That predicate is wrong in two ways that Smithy is explicit about, and having it
/// spelled three times meant fixing it in three places:
///
/// * **`@default: null` means the member has NO default and is optional.** A JSON `null`
///   satisfies `contains_key`, so such a member was generated non-`Option`, zero-filled
///   on absence, and then rejected by its own `@range`.
/// * **`@addedDefault` marks a default added after the fact**, and Smithy keeps such a
///   member optional precisely because older clients omit it. It was never read at all.
///
/// Returning the value rather than a bool is what lets the single production call site
/// materialize it (see [`default_member_fallback_expr`]) instead of re-fetching it.
///
/// Why this fixes anything on its own: a member with no usable default and no
/// `@required` becomes `Option<T>`, and **the generated serializer writes a field only
/// `if let Some(..)`** — so the validating walk never sees an absent member. Absent
/// `Option` is structurally safe.
fn usable_default(
    member_traits: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<&serde_json::Value> {
    let traits = member_traits?;
    if traits.contains_key("smithy.api#addedDefault") {
        return None;
    }
    match traits.get("smithy.api#default") {
        Some(serde_json::Value::Null) | None => None,
        Some(value) => Some(value),
    }
}

/// The declared default as a Rust literal, or `None` when it cannot be materialized and
/// the type zero must stand in.
///
/// Returns `None` — deliberately, with the reason per arm — for:
///
/// * a value that does not fit the field's width (a model defect; see
///   [`default_member_fallback_expr`]);
/// * a non-finite float (`Literal::f32_suffixed` panics at codegen, so the value cannot
///   be emitted as a literal at all);
/// * an `Enum`, `Blob`, `Document`, `List`, `Map`, `Structure`, `Union` or `Other`
///   target, each for its own reason below;
/// * `Timestamp`, which the caller already handles.
fn materialized_default_expr(
    kind: &codegen::ShapeKind<'_>,
    default_value: Option<&serde_json::Value>,
) -> Option<proc_macro2::TokenStream> {
    let value = default_value?;
    /// A whole-number default, however the model encoded it.
    ///
    /// `as_i64` alone returns `None` for a JSON float, and `serde_json` distinguishes
    /// `1` from `1.0` at parse time — so a model writing `@default: 1.0` on an integer
    /// member would fall back to the type zero and reintroduce the false 400 this
    /// materialization exists to prevent. The fractional check keeps `1.5` out, and the
    /// range check keeps the `as i64` cast from saturating.
    fn whole(value: &serde_json::Value) -> Option<i64> {
        if let Some(n) = value.as_i64() {
            return Some(n);
        }
        let f = value.as_f64()?;
        (f.fract() == 0.0 && f >= (i64::MIN as f64) && f < 9_223_372_036_854_775_808.0)
            .then(|| f as i64)
    }
    match kind {
        // Integers: range-check against the FIELD width before emitting. `as` would
        // wrap (`200 as i8` -> -56) or saturate, either of which emits a confidently
        // wrong value rather than falling back to a visible zero.
        codegen::ShapeKind::Byte => i8::try_from(whole(value)?).ok().map(|n| quote! { #n }),
        codegen::ShapeKind::Short => i16::try_from(whole(value)?).ok().map(|n| quote! { #n }),
        codegen::ShapeKind::Integer => i32::try_from(whole(value)?).ok().map(|n| quote! { #n }),
        codegen::ShapeKind::Long => whole(value).map(|n| quote! { #n }),
        // Floats: a non-finite value must not reach `Literal::f*_suffixed`, which
        // asserts finiteness and PANICS AT CODEGEN, failing the whole crate's build.
        // Smithy permits NaN/Infinity as a default, so this is reachable from a model.
        codegen::ShapeKind::Float => {
            // Check finiteness AFTER narrowing: a large-but-finite `f64` becomes
            // `f32::INFINITY`, which `Literal::f32_suffixed` would panic on.
            let wide = value.as_f64()?;
            let n = wide as f32;
            (wide.is_finite() && n.is_finite()).then(|| quote! { #n })
        }
        codegen::ShapeKind::Double => {
            let n = value.as_f64()?;
            n.is_finite().then(|| quote! { #n })
        }
        codegen::ShapeKind::Boolean => value.as_bool().map(|b| quote! { #b }),
        // An `enum` shape is generated as a `String` field, so its default
        // materializes exactly like one. It MUST materialize: the type zero is
        // `""`, which no enum's value set contains, so a member left to the zero
        // is rejected by the enum check the moment that check exists — a false
        // 400 on a request that legally omitted the member.
        codegen::ShapeKind::String | codegen::ShapeKind::Enum => {
            let s = value.as_str()?;
            Some(quote! { #s.to_string() })
        }
        // BigInteger / BigDecimal are parsed from a string at runtime by the codecs, and
        // constructing one at codegen would need that parse here. `@range` on them is
        // unenforced today anyway, so the zero cannot produce a false 400 — revisit
        // together with that.
        codegen::ShapeKind::BigInteger | codegen::ShapeKind::BigDecimal => None,
        // A blob default is base64 in the model; decoding it here would duplicate the
        // codec. Falls back to the empty blob, which is what every declared blob
        // default is today — a non-empty one needs this arm implemented.
        codegen::ShapeKind::Blob => None,
        // Timestamp is the caller's existing arm.
        codegen::ShapeKind::Timestamp => None,
        // Aggregates: Smithy only permits an empty list/map default, which IS the type
        // zero, so there is nothing to materialize.
        codegen::ShapeKind::List { .. } | codegen::ShapeKind::Map { .. } => None,
        codegen::ShapeKind::Document
        | codegen::ShapeKind::Structure
        | codegen::ShapeKind::Union
        | codegen::ShapeKind::Other(_) => None,
    }
}

/// The expression filling a non-`Option` `@default` member the client omitted.
///
/// # Materialize the DECLARED value, not the type's zero
///
/// Returning the TYPE's zero here instead would make a member modeled `@default: 10`
/// arrive as **0**. That is a wire-visible lie on its own, and it becomes a false 400 as
/// soon as anything validates the member: every SDK paginator omits `maxResults`,
/// `@range(min: 1)` rejects the 0, and the caller gets a 400 on the FIRST page of a List
/// call for a value they never sent.
///
/// Smithy's own server protocol test
/// `RestJsonServerPopulatesDefaultsWhenMissingInRequestBody` asserts
/// `defaultInteger: Integer = 10` arrives as 10, so this is a spec rule rather than a
/// judgment call. Materialize verbatim and let it flow through the existing walk: no
/// clamping, and **no exemption for defaulted members in the validator** — exempting
/// them would trade a false 400 for a false 200 and stop catching the values a client
/// actually sends.
///
/// # The one exemption, and why it is not "clamping"
///
/// A declared default that does not FIT its own field width falls back to the type zero,
/// exactly as the `Timestamp` arm below already did. That fires only on a model defect
/// (a `@default` incompatible with its own shape, which Smithy forbids), and the
/// alternative is worse: `200 as i8` wraps to `-56` and an out-of-range `as i64`
/// saturates, so a narrowing cast would emit a confidently wrong value.
///
/// # Two codegen-time hazards
///
/// * A non-finite float must go through a `f32::NAN` / `INFINITY` path.
///   `Literal::f32_suffixed` **asserts finiteness and panics at codegen time**, which
///   fails the build of an entire crate rather than one operation.
/// * An enum-targeted default materializes its modeled wire string, in the same arm
///   as `string`. It MUST: the type zero is `""`, which is in no enum's value set,
///   so a member left to the zero is refused by the enum check on a request that
///   legally omitted it. A default that is not a string against an enum shape is a
///   model defect and still falls back to the zero, rather than emitting a literal
///   of the wrong Rust type.
fn default_member_fallback_expr(
    kind: &codegen::ShapeKind<'_>,
    default_value: Option<&serde_json::Value>,
) -> proc_macro2::TokenStream {
    if let Some(expr) = materialized_default_expr(kind, default_value) {
        return expr;
    }
    if !matches!(kind, codegen::ShapeKind::Timestamp) {
        return quote! { ::std::default::Default::default() };
    }
    match default_value.and_then(|v| v.as_f64()) {
        // Whole seconds, and exactly representable as `i64`. The range guard
        // matters because `as i64` saturates rather than failing, which would
        // silently turn an out-of-range modeled default into `i64::MAX`.
        //
        // The upper bound is the exclusive constant 2^63 rather than
        // `i64::MAX as f64`: `i64::MAX` (2^63 - 1) is not representable in `f64`
        // and rounds UP to 2^63, so comparing against it would admit exactly
        // 2^63 and saturate. `i64::MIN` is exactly -2^63, so an inclusive lower
        // bound is correct.
        Some(secs)
            if secs.fract() == 0.0
                && secs >= (i64::MIN as f64)
                && secs < 9_223_372_036_854_775_808.0 =>
        {
            let whole = secs as i64;
            quote! { ::aws_smithy_types::DateTime::from_secs(#whole) }
        }
        Some(secs) => quote! { ::aws_smithy_types::DateTime::from_secs_f64(#secs) },
        None => quote! { ::aws_smithy_types::DateTime::from_secs(0) },
    }
}

/// Convert an error shape/variant name to its generated schema constant name
/// (e.g. `NotFoundException` -> `SCHEMA_NOT_FOUND_EXCEPTION`).
///
/// The name must be an injective function of the Rust error type name. The
/// constant is emitted once per error struct and referenced by every
/// `operation_error` enum arm in the same module, so two distinct error shapes
/// that map to one constant both collide (`E0428`) and mis-bind the surviving
/// arm to the wrong schema. The full name is therefore significant: a trailing
/// `Exception` is part of it, because AWS ships both spellings side by side in a
/// single namespace (CloudWatch declares `ResourceNotFound` *and*
/// `ResourceNotFoundException`).
fn error_name_to_schema_const(name: &str) -> String {
    format!("SCHEMA_{}", to_snake_case(name).to_uppercase())
}

/// Convert a Smithy enum member name to its expected Rust PascalCase form.
///
/// Smithy enum member names appear in the model in two flavours: AWS service
/// models like KMS use PascalCase (`PendingDeletion`); most newer models use
/// SCREAMING_SNAKE_CASE (`PENDING_DELETION`). For SCREAMING_SNAKE we
/// title-case each underscore-separated word; for PascalCase we pass the name
/// through unchanged. Inputs without underscores that mix case are treated as
/// PascalCase.
fn screaming_snake_to_pascal(s: &str) -> String {
    let has_underscore = s.contains('_');
    let has_lower = s.chars().any(|c| c.is_lowercase());
    let has_upper = s.chars().any(|c| c.is_uppercase());
    if !has_underscore && has_lower && has_upper {
        // Already PascalCase (or some other CamelCase form). Pass through.
        return s.to_string();
    }
    if !has_underscore && has_lower && !has_upper {
        // All-lowercase single word (e.g., "healthcheck") — capitalize first letter.
        let mut chars = s.chars();
        return match chars.next() {
            Some(c) => c.to_uppercase().to_string() + chars.as_str(),
            None => String::new(),
        };
    }
    s.split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().to_string() + &chars.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect()
}

// =============================================================================
// Empty (Unit) struct emission — no model lookup needed
// =============================================================================

fn emit_empty_struct(item: &syn::ItemStruct, shape_id: &str) -> syn::Result<TokenStream> {
    let name = &item.ident;
    let namespace = shape_id.split('#').next().unwrap_or("smithy.unknown");
    let local_name = shape_id.split('#').nth(1).unwrap_or("Unknown");
    let schema_const_name =
        quote::format_ident!("{}_SCHEMA", to_snake_case(&name.to_string()).to_uppercase());

    // Construct the empty value field-by-field (there are no fields) rather than
    // via `Self::default()`, so the generated deserializer never requires the
    // struct to implement `Default`. A unit struct (`struct Foo;`) is built with
    // `Self`; an empty named struct (`struct Foo {}`) with `Self {}`.
    let empty_ctor = match &item.fields {
        syn::Fields::Named(_) => quote! { Self {} },
        _ => quote! { Self },
    };

    Ok(quote! {
        #item

        #[allow(non_upper_case_globals)]
        impl #name {
            pub const SCHEMA: ::aws_smithy_schema::Schema =
                ::aws_smithy_schema::Schema::new_struct(
                    ::aws_smithy_schema::ShapeId::from_static(
                        #shape_id,
                        #namespace,
                        #local_name,
                    ),
                    ::aws_smithy_schema::ShapeType::Structure,
                    &[],
                );
        }

        pub static #schema_const_name: &::aws_smithy_schema::Schema = &#name::SCHEMA;

        impl ::aws_smithy_schema::serde::SerializableStruct for #name {
            fn serialize_members(
                &self,
                __s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
            ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
                Ok(())
            }
        }

        impl ::aws_smithy_schema_server::DeserializableStruct for #name {
            fn deserialize(
                __d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
            ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
                __d.read_struct(&#name::SCHEMA, &mut |_, _| Ok(()))?;
                Ok(#empty_ctor)
            }
        }
    }
    .into())
}

// Struct impl emission (Schema + Serialize + Deserialize)
// =============================================================================

fn emit_struct_impls(
    input: &DeriveInput,
    shape_id: &str,
    shape: &serde_json::Value,
    model: &HashMap<String, serde_json::Value>,
    unsupported_fields: &HashSet<String>,
    skip_validation_fields: &HashSet<String>,
) -> syn::Result<proc_macro2::TokenStream> {
    use proc_macro2::TokenStream as TS;

    let name = &input.ident;
    let Data::Struct(data_struct) = &input.data else {
        return Err(syn::Error::new_spanned(input, "expected a struct"));
    };
    let Fields::Named(named) = &data_struct.fields else {
        return Err(syn::Error::new_spanned(input, "expected named fields"));
    };

    let smithy_members = shape
        .get("members")
        .and_then(|m| m.as_object())
        .ok_or_else(|| syn::Error::new_spanned(input, "Shape has no members"))?;

    let rust_fields: HashMap<String, &syn::Field> = named
        .named
        .iter()
        .map(|f| {
            let name = f.ident.as_ref().unwrap().to_string();
            let name = name.strip_prefix("r#").unwrap_or(&name).to_string();
            (name, f)
        })
        .collect();

    let mut member_static_decls: Vec<TS> = Vec::new();
    let mut member_static_refs: Vec<TS> = Vec::new();
    let mut serialize_arms: Vec<TS> = Vec::new();
    let mut deserialize_arms: Vec<TS> = Vec::new();
    // Field-by-field deserialization state. Each member is read into a local
    // `Option<#inner_ty>` slot (`field_slot_decls`) during the structural walk,
    // and the final struct is built from the collected slots (`field_ctor_entries`)
    // — so the generated `deserialize` never requires `Self: Default`.
    let mut field_slot_decls: Vec<TS> = Vec::new();
    let mut field_ctor_entries: Vec<TS> = Vec::new();
    // Module-level `LazyLock<TraitMap>` statics carrying captured constraint
    // traits (e.g. `@length`). These cannot live inside an `impl` block (no
    // `static` items there), so they are emitted at module scope and referenced
    // by the member schema's `.with_traits(&...)` chain. See the constraint-
    // traits framework: the generic `ValidatingSerializer` walk reads these at
    // the dispatch boundary.
    let mut member_traits_statics: Vec<TS> = Vec::new();
    // These statics must live outside the `impl` (a `LazyLock` cannot be an
    // associated item), which puts them at module scope alongside every OTHER
    // struct's. Namespacing them by a name BUILT from the struct and member — the
    // previous scheme — is not injective: joining two snake_cased names with `_`
    // cannot be undone, so `Environment.templateName` and `EnvironmentTemplate.name`
    // produced one identifier and the module failed to compile. A module named after
    // the struct's own ident is injective for free, because Rust already guarantees
    // one type name per module, and it lets the member name stand alone inside.
    let traits_mod = quote::format_ident!("__member_traits_{}", name);

    let namespace = shape_id.split('#').next().unwrap_or("smithy.unknown");
    let local_name = shape_id.split('#').nth(1).unwrap_or("Unknown");

    // Constraint traits are captured for EVERY struct, with no "is this an input?"
    // gate. Capturing is not enforcing: the only thing that enforces them is
    // `validate_to_message`, and `smithy_service` emits exactly one call to it —
    // on the deserialized INPUT, before the handler runs (see the dispatch arm in
    // `emit_operation_dispatch`). An output struct's captured traits are therefore
    // inert, so there is nothing for a gate to protect.
    //
    // A gate here was actively harmful, because "is an input" is not a property a
    // struct has in isolation:
    //
    //   * Keyed on the `@input` trait, it dropped every constraint for the 7.4% of
    //     operation input shapes that do not carry that annotation. `@input` is a
    //     Smithy 2.0 convention meaning "this shape exists solely as this
    //     operation's input" and is orthogonal to validation.
    //   * Keyed on "some operation targets this shape as its input", it still
    //     dropped every constraint on a struct NESTED inside an input — which is
    //     30% of all constrained members reachable from an input. A nested struct
    //     is not itself an operation input, so the gate was false for it, and a
    //     `@pattern` one level down was silently unenforced while the identical
    //     `@pattern` on a top-level member was enforced.
    //
    // Real services validate nested members and report the full path
    // (`transactItems.1.member.get.tableName`), which is why `ValidatingSerializer`
    // maintains a path stack in the first place. The walk was always able to reach
    // these members; only the trait capture was missing.

    let mut emitted_index: usize = 0;
    for (smithy_name, smithy_member) in smithy_members {
        if unsupported_fields.contains(smithy_name) {
            continue;
        }
        // HTTP-bound members (httpQuery, httpLabel, httpHeader) ARE emitted in the
        // schema and SerializableStruct/DeserializableStruct impls. The protocol's
        // HttpBindingDeserializer / HttpBindingSerializer route reads/writes for
        // these members to/from request path/query/headers based on the schema
        // traits we attach below (`with_http_label()`, `with_http_query(...)`,
        // `with_http_header(...)`). The body codec never sees them.
        let member_traits = smithy_member.get("traits").and_then(|t| t.as_object());
        let rust_name = to_snake_case(smithy_name);
        let rust_field = rust_fields.get(&rust_name).ok_or_else(|| {
            syn::Error::new_spanned(
                input,
                format!("internal: field '{rust_name}' missing after validation"),
            )
        })?;

        let field_ident = rust_field.ident.as_ref().unwrap();
        let field_ty = &rust_field.ty;

        let kind = codegen::classify_member(smithy_member, model).ok_or_else(|| {
            syn::Error::new_spanned(
                rust_field,
                format!("internal: cannot classify Smithy member '{smithy_name}'"),
            )
        })?;
        let shape_type = codegen::shape_type_ident(&kind);

        let static_name = quote::format_ident!("M_{}", to_snake_case(smithy_name).to_uppercase());
        let idx_lit = syn::LitInt::new(&emitted_index.to_string(), proc_macro2::Span::call_site());

        // A member's own `@xmlName` renames the element that member occupies —
        // the wrapper of a wrapped list, each repeated element of a flattened
        // one, and the single element of any other member. The XML codec reads
        // it from `Schema::xml_name()` on BOTH sides (the deserializer matches an
        // element by member name or `xml_name`), so baking it here keeps read and
        // write symmetric. A list's ITEM name is a different name and travels on
        // `XmlListTrait` below, so the two never contend for this one slot.
        //
        // `json_name_chain` bakes the independent `@jsonName` chain for the JSON
        // wire; see that helper for how the two names coexist.
        let xml_name_chain = xml_name_chain(member_traits);
        let json_name_chain = json_name_chain(member_traits);

        // HTTP binding traits on the member schema. The protocol uses these
        // at runtime to decide whether a member belongs in the URI path,
        // query string, headers, or body.
        let http_binding_chain = if let Some(traits) = member_traits {
            if traits.contains_key("smithy.api#httpLabel") {
                quote! { .with_http_label() }
            } else if let Some(q) = traits.get("smithy.api#httpQuery") {
                let name = q.as_str().unwrap_or(smithy_name);
                quote! { .with_http_query(#name) }
            } else if let Some(h) = traits.get("smithy.api#httpHeader") {
                let name = h.as_str().unwrap_or(smithy_name);
                quote! { .with_http_header(#name) }
            } else if let Some(p) = traits.get("smithy.api#httpPrefixHeaders") {
                let prefix = p.as_str().unwrap_or("");
                quote! { .with_http_prefix_headers(#prefix) }
            } else if traits.contains_key("smithy.api#httpQueryParams") {
                quote! { .with_http_query_params() }
            } else if traits.contains_key("smithy.api#httpResponseCode") {
                quote! { .with_http_response_code() }
            } else if traits.contains_key("smithy.api#httpPayload") {
                quote! { .with_http_payload() }
            } else {
                quote! {}
            }
        } else {
            quote! {}
        };

        // Capture every supported constraint trait (`@length`, `@range`,
        // `@pattern`, `@enum`, `@uniqueItems`) as runtime-traversable data on
        // the member schema's generic TraitMap. The dispatch-boundary
        // `ValidatingSerializer` walk reads these and enforces them; codegen
        // only *captures* the data — it bakes in no validation logic.
        let constraints = if skip_validation_fields.contains(smithy_name) {
            codegen::Constraints::default()
        } else {
            codegen::extract_constraints(smithy_member, model)
        };

        // Emit a `LazyLock<TraitMap>` static for this member's captured
        // constraint traits and chain `.with_traits(&...)` onto the schema.
        // Module-level statics must be unique across all structs in the module,
        // so namespace by the struct name (member schema consts live inside the
        // `impl` and need no such prefix).
        let traits_static_name =
            quote::format_ident!("M_{}_TRAITS", to_snake_case(smithy_name).to_uppercase());
        let mut trait_inserts = codegen::emit_constraint_trait_inserts(&constraints, &kind);

        // Carry `@ec2QueryName` on the member's generic TraitMap; the rationale
        // and the trait it emits live on `ec2_query_name_insert`.
        if let Some(insert) = ec2_query_name_insert(member_traits) {
            trait_inserts.push(insert);
        }

        // XML list layout for a list-typed member: the `@xmlFlattened` flag plus
        // the item element name from the list member's `@xmlName`. Both are
        // needed by the restXml writer and neither is reachable at runtime
        // otherwise — `aws-smithy-schema` records `@xmlFlattened` but exposes no
        // getter, and a `Schema` has a single `xml_name` slot which the member's
        // own name already occupies. `@xmlFlattened` is legal on the member or on
        // the list shape itself, so check both. Emitted for input AND output
        // structs, because a wrong list layout makes a RESPONSE unparseable.
        // (Constraint traits are now captured on every struct too; the difference
        // is that a list layout is READ on the response path while a constraint is
        // only ever read on the request path.)
        if let codegen::ShapeKind::List { .. } = &kind {
            let (flattened, item_name) = xml_list_layout(smithy_member, model);
            let item_expr = match item_name {
                Some(n) => quote! { ::std::option::Option::Some(#n) },
                None => quote! { ::std::option::Option::None },
            };
            trait_inserts.push(quote! {
                m.insert(::std::boxed::Box::new(
                    ::aws_smithy_schema_server::xml::XmlListTrait::new(#flattened, #item_expr)
                ));
            });
        }

        // Carry the map member's `@xmlFlattened` flag on its TraitMap via
        // [`XmlMapTrait`]; `xml_map_layout` resolves it. Emitted for input AND
        // output structs, because a wrong map layout makes a RESPONSE
        // unparseable, not just a request.
        if let codegen::ShapeKind::Map { .. } = &kind {
            let flattened = xml_map_layout(smithy_member, model);
            trait_inserts.push(quote! {
                m.insert(::std::boxed::Box::new(
                    ::aws_smithy_schema_server::xml::XmlMapTrait::new(#flattened)
                ));
            });
        }

        // For a collection member whose scalar *leaf* shape carries its own
        // constraints (e.g. `list<TableName>` where `TableName` is
        // `@length`-constrained), capture those in a separate trait-map static
        // and attach them to this collection member via an
        // `ElementConstraintsTrait` wrapper. The validating walk runs them on
        // each scalar element/value at the element's path. This is additive: no
        // codec reads the wrapper, so serialization is unaffected. Map *keys*
        // are not captured.
        if !skip_validation_fields.contains(smithy_name) {
            if let Some((leaf_kind, leaf_constraints)) =
                codegen::extract_element_constraints(&kind, smithy_member, model)
            {
                let inner_inserts =
                    codegen::emit_constraint_trait_inserts(&leaf_constraints, leaf_kind);
                if !inner_inserts.is_empty() {
                    let elem_traits_static_name = quote::format_ident!(
                        "M_{}_ELEM_TRAITS",
                        to_snake_case(smithy_name).to_uppercase()
                    );
                    member_traits_statics.push(quote! {
                        #[allow(non_upper_case_globals)]
                        pub(super) static #elem_traits_static_name:
                            ::std::sync::LazyLock<::aws_smithy_schema::TraitMap> =
                            ::std::sync::LazyLock::new(|| {
                                let mut m = ::aws_smithy_schema::TraitMap::new();
                                #(#inner_inserts)*
                                m
                            });
                    });
                    trait_inserts.push(quote! {
                        m.insert(::std::boxed::Box::new(
                            ::aws_smithy_schema_server::constraints::ElementConstraintsTrait::new(
                                &#traits_mod::#elem_traits_static_name
                            )
                        ));
                    });
                }
            }
        }

        let traits_chain = if !trait_inserts.is_empty() {
            member_traits_statics.push(quote! {
                #[allow(non_upper_case_globals)]
                pub(super) static #traits_static_name:
                    ::std::sync::LazyLock<::aws_smithy_schema::TraitMap> =
                    ::std::sync::LazyLock::new(|| {
                        let mut m = ::aws_smithy_schema::TraitMap::new();
                        #(#trait_inserts)*
                        m
                    });
            });
            quote! { .with_traits(&#traits_mod::#traits_static_name) }
        } else {
            quote! {}
        };

        // Key/value (map) or element (list) sub-member schemas. The XML codec
        // reads the element names for a map's `key`/`value` (and a list's item)
        // off these on both the serialize and deserialize paths, so a modeled
        // `@xmlName` on an inner member reaches the wire. A scalar member emits
        // nothing here and keeps the codec's Smithy-default element names.
        let shape_id_prefix = format!("{namespace}#{local_name}${smithy_name}");
        let (collection_sub_decls, collection_members_chain) = map_list_member_schema(
            &static_name,
            &shape_id_prefix,
            namespace,
            smithy_member,
            model,
        );

        member_static_decls.extend(collection_sub_decls);
        member_static_decls.push(quote! {
            const #static_name: ::aws_smithy_schema::Schema =
                ::aws_smithy_schema::Schema::new_member(
                    ::aws_smithy_schema::ShapeId::from_static(
                        concat!(#namespace, "#", #local_name, "$", #smithy_name),
                        #namespace,
                        #smithy_name,
                    ),
                    #shape_type,
                    #smithy_name,
                    #idx_lit,
                )#xml_name_chain #json_name_chain #http_binding_chain #collection_members_chain #traits_chain;
        });
        member_static_refs.push(quote! { &Self::#static_name });

        let is_option = codegen::unwrap_option(field_ty).is_some();
        let inner_ty: &syn::Type = codegen::unwrap_option(field_ty).unwrap_or(field_ty);

        let member_expr = quote! { &Self::#static_name };

        // Read arm. Deserialization is now purely structural (shape/type); all
        // constraint enforcement happens in the framework's validating walk at
        // the dispatch boundary, driven by the captured trait maps above. No
        // inline `if len < N` checks are emitted here.
        let read_expr = codegen::emit_read(&kind, inner_ty, &member_expr, &quote! { __d })?;
        // Collect this member into a local `Option<#inner_ty>` slot instead of
        // writing into a pre-`default()`ed `__out`. The struct is assembled from
        // these slots after the read (see the `deserialize` impl), which keeps a
        // `@required` member as a bare (non-Option) field without ever needing
        // `Self: Default` — e.g. a required `aws_smithy_types::DateTime`, which
        // has no `Default`.
        let slot_ident = quote::format_ident!("__field_{}", emitted_index);
        field_slot_decls.push(quote! {
            let mut #slot_ident: ::std::option::Option<#inner_ty> =
                ::std::option::Option::None;
        });
        let arm_idx = syn::LitInt::new(&emitted_index.to_string(), proc_macro2::Span::call_site());
        // Reading a member may surface a required member absent from a shape
        // NESTED under it. Name this member as the error passes back out, so the
        // trail reads from the operation input inwards; a closure gives the read
        // one exit point to attach it to. Errors of any other kind pass through.
        // `read_expr` ends in `?` (`codegen::emit_read`), so bind it to a local
        // before wrapping it: `Ok(read?)` trips `clippy::needless_question_mark`
        // in the generated crate.
        deserialize_arms.push(quote! {
            Some(#arm_idx) => {
                let __read = (|| -> ::std::result::Result<
                    #inner_ty,
                    ::aws_smithy_schema::serde::SerdeError,
                > {
                    let __value = #read_expr;
                    ::std::result::Result::Ok(__value)
                })();
                #slot_ident = ::std::option::Option::Some(__read.map_err(|__e| {
                    ::aws_smithy_schema_server::within_member(__e, #smithy_name)
                })?);
            }
        });
        if is_option {
            field_ctor_entries.push(quote! { #field_ident: #slot_ident });
        } else {
            // Distinguish @required (hard error if absent) from @default (use
            // type default if absent). Both are non-Option fields, but @default
            // members may be omitted by clients — AWS fills in the default
            // server-side, and so should we.
            // `usable_default` rather than `contains_key`: `@default: null` means the
            // member has NO default, and `@addedDefault` keeps it optional.
            let declared_default = usable_default(member_traits);
            let has_required_trait = member_traits
                .map(|t| t.contains_key("smithy.api#required"))
                .unwrap_or(false);

            if has_required_trait || declared_default.is_none() {
                // Truly required — error if absent. The error names the member and
                // nothing else; the enclosing members prepend themselves as it
                // unwinds, and the protocol boundary words the refusal.
                field_ctor_entries.push(quote! {
                    #field_ident: #slot_ident.ok_or_else(|| {
                        ::aws_smithy_schema_server::missing_member(#smithy_name)
                    })?
                });
            } else {
                // `@default` but not `@required`: clients may omit the member and
                // AWS fills the modeled default in server-side, so we do too.
                // The fallback must not impose a `Default` bound on the field
                // type -- a timestamp has none. See
                // `default_member_fallback_expr`.
                let fallback = default_member_fallback_expr(&kind, declared_default);
                field_ctor_entries.push(quote! {
                    #field_ident: #slot_ident.unwrap_or_else(|| #fallback)
                });
            }
        }

        // Write arm
        let write_value_expr = if is_option {
            quote! { __v }
        } else {
            quote! { (&self.#field_ident) }
        };
        let write_stmt = codegen::emit_write(
            &kind,
            inner_ty,
            &member_expr,
            &write_value_expr,
            &quote! { __s },
        )?;
        if is_option {
            serialize_arms.push(quote! {
                if let Some(__v) = self.#field_ident.as_ref() {
                    #write_stmt
                }
            });
        } else {
            serialize_arms.push(write_stmt);
        }

        emitted_index += 1;
    }

    let struct_id_lit = shape_id;
    let schema_const_name =
        quote::format_ident!("{}_SCHEMA", to_snake_case(&name.to_string()).to_uppercase());

    // If this struct is the input or output of an operation that has an
    // `@http` trait, attach the operation's http info onto the struct's
    // SCHEMA so the protocol can construct the URI / status code without
    // looking the operation up separately.
    //
    // The operation ID does not appear on the struct itself, so we scan the
    // model for an operation whose `input.target` or `output.target` equals
    // this shape's ID.
    // A struct-level `@xmlName` renames the shape's own XML element, which for
    // an operation output IS the response root. S3 leans on this heavily —
    // `ListObjectsV2Output` is `@xmlName("ListBucketResult")`,
    // `CompleteMultipartUploadOutput` is `CompleteMultipartUploadResult`,
    // `GetBucketLocationOutput` is `LocationConstraint`, and 27 of S3's 70 output
    // shapes rename this way. The XML codec already honours
    // `Schema::xml_name()`; without baking it here the writer falls back to the
    // shape name and every SDK looks for a root element that never arrives.
    let struct_xml_name_chain = match shape
        .get("traits")
        .and_then(|t| t.get("smithy.api#xmlName"))
        .and_then(|v| v.as_str())
    {
        Some(xn) => quote! { .with_xml_name(#xn) },
        None => quote! {},
    };

    // Bake `method` + `uri` onto this shape's `Schema` only when it is the
    // INPUT of an operation. The request path reads `input_schema.http().uri()`
    // to match a REST route and fill `@httpLabel` placeholders, so an input
    // shape must carry it. The success status is NOT baked here: it is an
    // operation fact (two operations may share one output shape yet return
    // different codes), carried on `Operation::SUCCESS_STATUS` and read from
    // there at serialize time — so an output shape gets no `@http` and the
    // status code is never attached to a shape at all. The code slot is left
    // `None`, which the schema reads as the unused default.
    let http_struct_chain = {
        let mut chain = quote! {};
        for (_op_name, op_shape) in model.iter() {
            if op_shape.get("type").and_then(|t| t.as_str()) != Some("operation") {
                continue;
            }
            let input_target = op_shape
                .get("input")
                .and_then(|i| i.get("target"))
                .and_then(|t| t.as_str());
            if input_target != Some(shape_id) {
                continue;
            }
            let http_trait = op_shape
                .get("traits")
                .and_then(|t| t.get("smithy.api#http"));
            if let Some(http) = http_trait {
                let method = http
                    .get("method")
                    .and_then(|m| m.as_str())
                    .unwrap_or("POST");
                let uri = http.get("uri").and_then(|u| u.as_str()).unwrap_or("/");
                chain = quote! {
                    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
                        #method,
                        #uri,
                        ::std::option::Option::None,
                    ))
                };
                break;
            }
        }
        chain
    };

    // Constraint trait maps referenced by member schemas via `.with_traits(&...)`.
    // They live outside the `impl` because `static` items (required for `LazyLock`)
    // cannot be associated items, and inside a module named after this struct so that
    // two structs in one module cannot collide. A struct with no constrained member
    // gets no module rather than an empty one.
    let traits_mod_def = if member_traits_statics.is_empty() {
        quote! {}
    } else {
        quote! {
            #[doc(hidden)]
            #[allow(non_snake_case)]
            mod #traits_mod {
                use super::*;
                #(#member_traits_statics)*
            }
        }
    };

    Ok(quote! {
        #traits_mod_def

        #[allow(non_upper_case_globals)]
        impl #name {
            #(#member_static_decls)*

            pub const SCHEMA: ::aws_smithy_schema::Schema =
                ::aws_smithy_schema::Schema::new_struct(
                    ::aws_smithy_schema::ShapeId::from_static(
                        #struct_id_lit,
                        #namespace,
                        #local_name,
                    ),
                    ::aws_smithy_schema::ShapeType::Structure,
                    &[#(#member_static_refs,)*],
                ) #struct_xml_name_chain #http_struct_chain;
        }

        pub static #schema_const_name: &::aws_smithy_schema::Schema = &#name::SCHEMA;

        impl ::aws_smithy_schema::serde::SerializableStruct for #name {
            fn serialize_members(
                &self,
                __s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
            ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
                #(#serialize_arms)*
                Ok(())
            }
        }

        impl ::aws_smithy_schema_server::DeserializableStruct for #name {
            fn deserialize(
                __d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
            ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
                #(#field_slot_decls)*
                __d.read_struct(&#name::SCHEMA, &mut |__member, __d| {
                    match __member.member_index() {
                        #(#deserialize_arms)*
                        _ => {}
                    }
                    Ok(())
                })?;
                Ok(Self {
                    #(#field_ctor_entries,)*
                })
            }
        }
    })
}

// =============================================================================
// Validation
// =============================================================================

fn validate_struct_against_shape(
    input: &DeriveInput,
    shape: &serde_json::Value,
    model: &HashMap<String, serde_json::Value>,
    unsupported_fields: &HashSet<String>,
) -> syn::Result<()> {
    let Data::Struct(data_struct) = &input.data else {
        return Err(syn::Error::new_spanned(input, "expected a struct"));
    };
    let Fields::Named(fields) = &data_struct.fields else {
        return Err(syn::Error::new_spanned(input, "expected named fields"));
    };

    let smithy_members = shape
        .get("members")
        .and_then(|m| m.as_object())
        .ok_or_else(|| syn::Error::new_spanned(input, "Shape has no members"))?;

    let mut rust_fields: HashMap<String, &syn::Field> = HashMap::new();
    for field in &fields.named {
        let field_name = field.ident.as_ref().unwrap().to_string();
        let field_name = field_name
            .strip_prefix("r#")
            .unwrap_or(&field_name)
            .to_string();
        rust_fields.insert(field_name, field);
    }

    for (smithy_name, smithy_member) in smithy_members {
        let rust_name = to_snake_case(smithy_name);
        if unsupported_fields.contains(smithy_name) {
            continue;
        }

        let Some(rust_field) = rust_fields.get(&rust_name) else {
            return Err(syn::Error::new_spanned(
                input,
                format!(
                    "Missing field for Smithy member '{}'. Add:\n    pub {}: {},\nor add \"{}\" to the unsupported list.",
                    smithy_name,
                    rust_name,
                    suggest_rust_type(smithy_member, model),
                    smithy_name
                ),
            ));
        };

        validate_field_type(rust_field, smithy_member, model)?;
        rust_fields.remove(&rust_name);
    }

    if let Some((rust_name, rust_field)) = rust_fields.into_iter().next() {
        let smithy_name = to_pascal_case(&rust_name);
        return Err(syn::Error::new_spanned(
            rust_field,
            format!(
                "Field '{}' does not correspond to any Smithy member. Remove it or add \"{}\" to the unsupported list.",
                rust_name, smithy_name
            ),
        ));
    }

    Ok(())
}

fn validate_field_type(
    field: &syn::Field,
    smithy_member: &serde_json::Value,
    model: &HashMap<String, serde_json::Value>,
) -> syn::Result<()> {
    let traits = smithy_member.get("traits").and_then(|t| t.as_object());
    // "Must be non-`Option`" is `@required` OR a USABLE `@default`. A `@default: null`
    // member, or one carrying `@addedDefault`, stays optional per Smithy — and this
    // predicate is what forces the author's field type, so getting it wrong here is what
    // made those members non-`Option` and therefore zero-filled in the first place.
    let is_required = traits
        .map(|t| t.contains_key("smithy.api#required"))
        .unwrap_or(false)
        || usable_default(traits).is_some();

    let target = smithy_member
        .get("target")
        .and_then(|t| t.as_str())
        .ok_or_else(|| syn::Error::new_spanned(field, "Smithy member missing target"))?;

    let target_shape = model.get(target);
    let is_option = is_option_type(&field.ty);

    if is_required && is_option {
        return Err(syn::Error::new_spanned(
            &field.ty,
            format!(
                "Field '{}' is @required or @default in Smithy, so it must not be Option<T>.",
                field.ident.as_ref().unwrap()
            ),
        ));
    }

    if !is_required && !is_option {
        return Err(syn::Error::new_spanned(
            &field.ty,
            format!(
                "Field '{}' is optional in Smithy, so it must be Option<T>.",
                field.ident.as_ref().unwrap()
            ),
        ));
    }

    if let Some(shape) = target_shape {
        let shape_type = shape.get("type").and_then(|t| t.as_str());
        match shape_type {
            Some("map") if !type_is_map(&field.ty, is_option) => {
                return Err(syn::Error::new_spanned(
                    &field.ty,
                    format!(
                        "Field '{}' should be a map type",
                        field.ident.as_ref().unwrap()
                    ),
                ));
            }
            Some("list") if !type_is_list(&field.ty, is_option) => {
                return Err(syn::Error::new_spanned(
                    &field.ty,
                    format!(
                        "Field '{}' should be a list type (Vec)",
                        field.ident.as_ref().unwrap()
                    ),
                ));
            }
            _ => {}
        }
    }

    Ok(())
}

// =============================================================================
// Model loading
// =============================================================================

// A parsed Smithy model: the shape id -> shape JSON map, shared via `Arc` so
// callers reuse one parsed copy instead of deep-cloning the ~1.4 MB shape map.
type SmithyModel = std::sync::Arc<HashMap<String, serde_json::Value>>;

// Process-global cache of parsed Smithy models, keyed by the resolved model
// path. rustc loads the proc-macro dylib once per crate compilation and drives
// every `#[smithy(...)]` expansion from that single process, so this cache is
// populated on the first invocation and reused (parse-free) by all the rest.
static MODEL_CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, SmithyModel>>> =
    std::sync::OnceLock::new();

/// Derives a service's SigV4 signing name from its model traits, shared by both
/// front doors (the server `smithy_service` macro and the client `smithy_client`
/// macro) so the extraction order lives in one place.
///
/// A signing name has three honest model sources, tried in order: the service's
/// `aws.auth#sigv4` `name`, then the `aws.api#service` `endpointPrefix`, then the
/// `aws.api#service` `arnNamespace`. Both front doors MUST derive the SAME name,
/// because the client signs under it and the server claims the request by
/// comparing it against the resolved route key — a model relying on the fallback
/// would silently fail scope matching if the two read different sources. So the
/// SAME chain serves both: `endpointPrefix` is AWS's conventional signing name
/// when `aws.auth#sigv4.name` is absent, and `arnNamespace` is the last-resort
/// source (it is an ARN-construction field that only coincidentally equals the
/// signing name, so it is tried only after `endpointPrefix`).
///
/// Only `on_missing` stays caller-supplied, because the two front doors keep
/// their own honest policy when the model declares neither source: the server
/// returns `String::default()` (the empty string, meaning "claims no request" —
/// a signing name is NOT derivable from the shape id, so none is fabricated),
/// while the client returns an `Err` reported against its own span. Returning
/// `syn::Result` lets the error carry that span; an enum could not.
///
/// The returned name is NOT normalized — the server trims and lowercases it to
/// match `resolve_route_key`, which the client must not do, so that step stays
/// at the server call site.
fn derive_signing_name(
    service_traits: Option<&serde_json::Map<String, serde_json::Value>>,
    on_missing: impl FnOnce() -> syn::Result<String>,
) -> syn::Result<String> {
    let from_service = |key: &str| {
        service_traits
            .and_then(|t| t.get("aws.api#service"))
            .and_then(|s| s.get(key))
            .and_then(|v| v.as_str())
    };
    service_traits
        .and_then(|t| t.get("aws.auth#sigv4"))
        .and_then(|s| s.get("name"))
        .and_then(|n| n.as_str())
        .or_else(|| from_service("endpointPrefix"))
        .or_else(|| from_service("arnNamespace"))
        .map(str::to_owned)
        .map(Ok)
        .unwrap_or_else(on_missing)
}

fn load_smithy_model(rel_path: &str) -> syn::Result<SmithyModel> {
    let cache = MODEL_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().unwrap().get(rel_path) {
        return Ok(hit.clone());
    }
    let parsed = std::sync::Arc::new(load_smithy_model_inner(rel_path)?);
    cache
        .lock()
        .unwrap()
        .insert(rel_path.to_string(), parsed.clone());
    Ok(parsed)
}

fn load_smithy_model_inner(rel_path: &str) -> syn::Result<HashMap<String, serde_json::Value>> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").map_err(|_| {
        syn::Error::new(proc_macro2::Span::call_site(), "CARGO_MANIFEST_DIR not set")
    })?;

    // First check CARGO_MANIFEST_DIR directly (for published crates that
    // bundle their model file), then walk up ancestors (for workspace builds
    // where the model lives in a sibling directory like `server-codegen/`).
    let model_path: std::path::PathBuf = std::path::Path::new(&manifest_dir)
        .ancestors()
        .map(|a| a.join(rel_path))
        .find(|c| c.exists())
        .ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!(
                    "Could not find Smithy model `{}` in any ancestor of `{}`",
                    rel_path, manifest_dir
                ),
            )
        })?;

    let json_str = std::fs::read_to_string(&model_path).map_err(|e| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            format!("Failed to read Smithy model at {:?}: {}", model_path, e),
        )
    })?;

    let model: serde_json::Value = serde_json::from_str(&json_str).map_err(|e| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            format!("Failed to parse Smithy model JSON: {}", e),
        )
    })?;

    let shapes = model
        .get("shapes")
        .and_then(|s| s.as_object())
        .ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                "Smithy model missing 'shapes' object",
            )
        })?;

    Ok(shapes.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

// =============================================================================
// Type helpers
// =============================================================================

fn is_option_type(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return segment.ident == "Option";
        }
    }
    false
}

fn type_is_map(ty: &Type, unwrap_option: bool) -> bool {
    let ty = if unwrap_option {
        unwrap_option_type(ty).unwrap_or(ty)
    } else {
        ty
    };
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            let s = segment.ident.to_string();
            return s == "HashMap" || s == "BTreeMap" || s == "Item";
        }
    }
    false
}

fn type_is_list(ty: &Type, unwrap_option: bool) -> bool {
    let ty = if unwrap_option {
        unwrap_option_type(ty).unwrap_or(ty)
    } else {
        ty
    };
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return segment.ident == "Vec";
        }
    }
    false
}

fn unwrap_option_type(ty: &Type) -> Option<&Type> {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Option" {
                if let PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(GenericArgument::Type(inner_ty)) = args.args.first() {
                        return Some(inner_ty);
                    }
                }
            }
        }
    }
    None
}

fn suggest_rust_type(
    smithy_member: &serde_json::Value,
    model: &HashMap<String, serde_json::Value>,
) -> String {
    let traits = smithy_member.get("traits").and_then(|t| t.as_object());
    // Must agree with `validate_field_type`'s predicate, or the diagnostic suggests a
    // type the validator then rejects.
    let is_required = traits
        .map(|t| t.contains_key("smithy.api#required"))
        .unwrap_or(false)
        || usable_default(traits).is_some();

    let target = smithy_member
        .get("target")
        .and_then(|t| t.as_str())
        .unwrap_or("unknown");

    let base_type = if let Some(shape) = model.get(target) {
        match shape.get("type").and_then(|t| t.as_str()) {
            Some("string") => "String",
            Some("boolean") => "bool",
            Some("integer") => "i32",
            Some("long") => "i64",
            Some("map") => "HashMap<String, AttributeValue>",
            Some("list") => "Vec<String>",
            Some("enum") => "EnumType",
            _ => "Type",
        }
    } else {
        "Type"
    };

    if is_required {
        base_type.to_string()
    } else {
        format!("Option<{}>", base_type)
    }
}

/// Convert a PascalCase or camelCase Smithy member name to snake_case.
///
/// Treats runs of uppercase letters as acronyms: `AWSAccountId` →
/// `aws_account_id`, `KeyId` → `key_id`, `KMSInternalException` →
/// `kms_internal_exception`. The rule: insert `_` before an uppercase
/// letter if the previous char is lowercase/digit, OR if the uppercase
/// letter starts a new word (i.e. is followed by a lowercase letter and
/// preceded by another uppercase letter).
fn to_snake_case(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut result = String::with_capacity(s.len() + 4);
    for i in 0..chars.len() {
        let ch = chars[i];
        if ch.is_uppercase() {
            let prev = if i > 0 { Some(chars[i - 1]) } else { None };
            let next = chars.get(i + 1).copied();
            let needs_underscore = match (prev, next) {
                (None, _) => false,
                (Some(p), _) if p.is_lowercase() || p.is_ascii_digit() => true,
                (Some(p), Some(n)) if p.is_uppercase() && n.is_lowercase() => true,
                _ => false,
            };
            if needs_underscore {
                result.push('_');
            }
            result.extend(ch.to_lowercase());
        } else {
            result.push(ch);
        }
    }
    result
}

/// Create a field identifier, using raw identifier syntax for Rust keywords.
#[allow(dead_code)]
fn make_field_ident(name: &str) -> proc_macro2::Ident {
    const KEYWORDS: &[&str] = &[
        "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn",
        "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
        "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe",
        "use", "where", "while", "async", "await", "dyn", "abstract", "become", "box", "do",
        "final", "macro", "override", "priv", "typeof", "unsized", "virtual", "yield", "try",
    ];
    // `_`, `self`, `Self`, `super` and `crate` are keywords that CANNOT be raw
    // identifiers: `proc_macro2::validate_ident_raw` panics on exactly this set, and
    // rustc rejects the spelling outright. Handing one to `new_raw` would abort the
    // build, which is strictly worse than the plain ident (a clear compile error at the
    // use site). A member with such a name has to be renamed or excluded by the
    // GENERATOR; there is nothing this helper can do about it.
    const NOT_RAW_IDENTIFIABLE: &[&str] = &["_", "self", "Self", "super", "crate"];
    if KEYWORDS.contains(&name) && !NOT_RAW_IDENTIFIABLE.contains(&name) {
        proc_macro2::Ident::new_raw(name, proc_macro2::Span::call_site())
    } else {
        quote::format_ident!("{}", name)
    }
}

fn to_pascal_case(s: &str) -> String {
    let mut result = String::new();
    let mut capitalize_next = true;
    for ch in s.chars() {
        if ch == '_' {
            capitalize_next = true;
        } else if capitalize_next {
            result.push(ch.to_uppercase().next().unwrap());
            capitalize_next = false;
        } else {
            result.push(ch);
        }
    }
    result
}

// =============================================================================
// smithy_service attribute macro (unchanged from before)
// =============================================================================

use darling::FromMeta;

#[derive(Debug, FromMeta)]
struct SmithyServiceOpts {
    /// The Smithy shape ID of the service (e.g.
    /// `"com.amazonaws.kms#TrentService"`).
    service: String,
    /// Path to the Rust module that holds operation input/output types.
    types: syn::Path,
    /// Path to the Rust module that holds the per-operation error enums
    /// (`<Op>Error`). Defaults to `crate::errors` — set this to override
    /// when your error module lives elsewhere (e.g. `crate::server::proto::errors`).
    #[darling(default)]
    errors: Option<syn::Path>,
    /// Workspace-relative path to the Smithy model JSON. Required.
    #[darling(default)]
    model: Option<String>,
    /// Smithy operation names (without namespace) that should be wired into
    /// the builder/dispatch — useful when you only implement a subset of a
    /// large service like KMS or DynamoDB. If omitted, every operation
    /// declared by the service is generated, which requires that
    /// `<Op>Input`/`<Op>Output`/`<Op>Error` types exist for each.
    #[darling(default)]
    operations: Vec<syn::LitStr>,
}

#[proc_macro_attribute]
pub fn smithy_service(attr: TokenStream, item: TokenStream) -> TokenStream {
    let item_struct = parse_macro_input!(item as syn::ItemStruct);
    match smithy_service_impl(attr, item_struct) {
        Ok(tokens) => tokens,
        Err(e) => emit_codegen_error(e, None).into(),
    }
}

/// Generate a typed client from a Smithy service shape — the client-side
/// counterpart of [`smithy_service`].
///
/// Applied to a unit struct, it emits one async method per listed operation,
/// each reusing the `#[smithy(...)]`-generated `<Op>Input`/`<Op>Output` schemas
/// and driving the real AWS SDK orchestrator via `amzn-smithy-client-runtime`.
///
/// ```ignore
/// #[smithy_macros::smithy_client(
///     service = "com.amazonaws.dynamodb#DynamoDB_20120810",
///     types = "crate::server::proto::operations",
///     model = "smithy-model/dynamodb-raw.json",
///     operations("PutItem", "GetItem"),
/// )]
/// pub struct DynamoDbMicroClient;
/// ```
///
/// The wire protocol, SigV4 signing name, and `x-amz-target` prefix are all read
/// from the model. Construct with a `smithy_client_runtime::Transport` so the
/// same client can point at the in-process emulator or at real DynamoDB.
#[proc_macro_attribute]
pub fn smithy_client(attr: TokenStream, item: TokenStream) -> TokenStream {
    let item_struct = parse_macro_input!(item as syn::ItemStruct);
    match client::smithy_client_impl(attr, item_struct) {
        Ok(tokens) => tokens,
        Err(e) => emit_codegen_error(e, None).into(),
    }
}

/// The Smithy `resource` lifecycle binding slots, in a fixed order. Each slot
/// (when present) binds exactly ONE operation via a `{ "target": ... }` object.
/// The order is arbitrary but STABLE so enumeration is deterministic.
const RESOURCE_LIFECYCLE_SLOTS: [&str; 6] = ["create", "put", "read", "update", "delete", "list"];

/// Collect every operation shape id bound to `service_shape`, unioning the
/// service's own top-level `operations` list with every operation reachable
/// through its `resources` tree.
///
/// Smithy binds operations to a `resource` shape via its lifecycle slots
/// (`create`/`put`/`read`/`update`/`delete`/`list`) and its `operations` /
/// `collectionOperations` lists, and resources nest via their own `resources`
/// list. Services like Lambda and Bedrock bind most or all of their operations
/// to resources, so wiring only `service.operations` misses them entirely (the
/// stub would compile but never route). This MUST stay byte-compatible with the
/// scaffold generator's `collect_operation_targets`: the generator emits one
/// stub per target and this macro wires the matching builder method, so any
/// divergence would leave a stub with no method (or a method with no stub).
///
/// Ordering is deterministic and ADDITIVE: `service.operations` come first in
/// their declared order, then the resource tree is walked depth-first in
/// declared order (each resource's lifecycle slots in the fixed
/// [`RESOURCE_LIFECYCLE_SLOTS`] order, then `operations`, then
/// `collectionOperations`, then nested `resources`). A target already seen is
/// skipped. A service that declares only `service.operations` (no `resources`)
/// therefore yields EXACTLY the previous list, in the same order.
fn collect_operation_targets(
    service_shape: &serde_json::Value,
    model: &HashMap<String, serde_json::Value>,
) -> Vec<String> {
    let mut ordered: Vec<String> = Vec::new();
    let mut seen_ops: HashSet<String> = HashSet::new();

    collect_targets_from_array(service_shape.get("operations"), &mut ordered, &mut seen_ops);

    let mut seen_resources: HashSet<String> = HashSet::new();
    collect_resource_targets(
        service_shape.get("resources"),
        model,
        &mut ordered,
        &mut seen_ops,
        &mut seen_resources,
    );

    ordered
}

/// Push each `{ "target": ... }` entry of the array under `arr` into `ordered`,
/// de-duped via `seen`, preserving declared order.
fn collect_targets_from_array(
    arr: Option<&serde_json::Value>,
    ordered: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    let Some(items) = arr.and_then(|a| a.as_array()) else {
        return;
    };
    for item in items {
        if let Some(target) = item.get("target").and_then(|t| t.as_str()) {
            if seen.insert(target.to_string()) {
                ordered.push(target.to_string());
            }
        }
    }
}

/// Depth-first walk of a `resources` array, collecting each resource's lifecycle
/// operations, instance `operations`, `collectionOperations`, and nested
/// `resources` into `ordered` (de-duped via `seen_ops`). `seen_resources` guards
/// against a resource being visited twice (repeated references or cycles).
fn collect_resource_targets(
    resources: Option<&serde_json::Value>,
    model: &HashMap<String, serde_json::Value>,
    ordered: &mut Vec<String>,
    seen_ops: &mut HashSet<String>,
    seen_resources: &mut HashSet<String>,
) {
    let Some(items) = resources.and_then(|a| a.as_array()) else {
        return;
    };
    for item in items {
        let Some(res_id) = item.get("target").and_then(|t| t.as_str()) else {
            continue;
        };
        if !seen_resources.insert(res_id.to_string()) {
            continue;
        }
        let Some(res_shape) = model.get(res_id) else {
            continue;
        };

        for slot in RESOURCE_LIFECYCLE_SLOTS {
            if let Some(target) = res_shape
                .get(slot)
                .and_then(|s| s.get("target"))
                .and_then(|t| t.as_str())
            {
                if seen_ops.insert(target.to_string()) {
                    ordered.push(target.to_string());
                }
            }
        }

        collect_targets_from_array(res_shape.get("operations"), ordered, seen_ops);
        collect_targets_from_array(res_shape.get("collectionOperations"), ordered, seen_ops);

        collect_resource_targets(
            res_shape.get("resources"),
            model,
            ordered,
            seen_ops,
            seen_resources,
        );
    }
}

/// Diagnostics computed by reconciling the collected operation targets with the
/// `operations(...)` allow-list, so [`smithy_service_impl`] can surface them at
/// the declaration site instead of letting a misconfiguration slip through and
/// resurface much later as a "method not found" / unroutable-operation failure.
struct OpTargetDiagnostics {
    /// The service declares NO operation binding whatsoever: it has neither an
    /// `operations` array (even an empty one) NOR any resource in its
    /// `resources` tree that contributes an operation. This is the exact case
    /// the pre-resource-traversal code hard-failed on ("Service shape has no
    /// operations array"); the resource-tree union accidentally relaxed it into
    /// a silent empty enumeration, so a typo'd/wrong `service` shape now compiles
    /// but wires nothing. The caller restores the hard `compile_error!`.
    ///
    /// A present-but-empty `operations: []` is DELIBERATELY still allowed (a
    /// valid do-nothing service — exactly what the scaffold emits for a fresh
    /// project; see the rpcv2-cbor / aws-query e2e crates), and resource-only
    /// services (Bedrock etc.) have a non-empty union, so neither trips this.
    no_op_binding: bool,
    /// Names listed in `operations(...)` that matched no discovered operation
    /// (sorted, de-duped). Each such entry silently wires nothing; the caller
    /// emits a compile-time warning at the declaration site.
    unmatched_filter: Vec<String>,
}

/// Reconcile the collected `operation_targets` (the union of `service.operations`
/// and the resource tree) with the `operations(...)` `op_filter` allow-list.
/// `declares_operations_array` is whether the service shape carries an
/// `operations` array at all (present-empty counts) — the discriminator between
/// a deliberate do-nothing service and a misconfiguration.
///
/// Pure and side-effect free so it is unit-testable without a model file — the
/// caller decides how to surface the two diagnostics ([`OpTargetDiagnostics`]).
fn diagnose_op_targets(
    operation_targets: &[String],
    op_filter: &HashSet<String>,
    declares_operations_array: bool,
) -> OpTargetDiagnostics {
    let discovered: HashSet<&str> = operation_targets
        .iter()
        .filter_map(|t| t.split('#').nth(1))
        .filter(|n| !n.is_empty())
        .collect();
    let mut unmatched_filter: Vec<String> = op_filter
        .iter()
        .filter(|f| !discovered.contains(f.as_str()))
        .cloned()
        .collect();
    unmatched_filter.sort();
    unmatched_filter.dedup();
    OpTargetDiagnostics {
        no_op_binding: operation_targets.is_empty() && !declares_operations_array,
        unmatched_filter,
    }
}

/// A wire protocol a service can declare. The rest of this crate matches on it to
/// pick the protocol's codec, error envelope and content type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ProtocolKind {
    AwsJson10,
    AwsJson11,
    RpcV2Cbor,
    AwsQuery,
    Ec2Query,
    RestXml,
    RestJson1,
}

impl ProtocolKind {
    /// The protocol's Smithy name, as diagnostics spell it.
    fn name(self) -> &'static str {
        match self {
            ProtocolKind::AwsJson10 => "awsJson1_0",
            ProtocolKind::AwsJson11 => "awsJson1_1",
            ProtocolKind::RpcV2Cbor => "rpcv2Cbor",
            ProtocolKind::AwsQuery => "awsQuery",
            ProtocolKind::Ec2Query => "ec2Query",
            ProtocolKind::RestXml => "restXml",
            ProtocolKind::RestJson1 => "restJson1",
        }
    }
}

impl std::fmt::Display for ProtocolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The Smithy trait a service shape carries to declare a protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ProtocolTraitId {
    AwsJson10,
    AwsJson11,
    RpcV2Cbor,
    AwsQuery,
    Ec2Query,
    RestXml,
    RestJson1,
}

impl ProtocolTraitId {
    /// The trait's absolute shape id, the key it has in a service's `traits`.
    fn shape_id(self) -> &'static str {
        match self {
            ProtocolTraitId::AwsJson10 => "aws.protocols#awsJson1_0",
            ProtocolTraitId::AwsJson11 => "aws.protocols#awsJson1_1",
            ProtocolTraitId::RpcV2Cbor => "smithy.protocols#rpcv2Cbor",
            ProtocolTraitId::AwsQuery => "aws.protocols#awsQuery",
            ProtocolTraitId::Ec2Query => "aws.protocols#ec2Query",
            ProtocolTraitId::RestXml => "aws.protocols#restXml",
            ProtocolTraitId::RestJson1 => "aws.protocols#restJson1",
        }
    }
}

/// The family of request headers that recognises a protocol's requests
/// (`WireProtocolKind::detect`), shared by every member of the family. Each
/// variant names the runtime's `WireProtocolKind` variant of the same name; this
/// crate cannot depend on the runtime crate, so it emits that variant by ident.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ProtocolRequestMarker {
    AwsJson,
    RpcV2Cbor,
    Query,
}

impl ProtocolRequestMarker {
    /// The ident of the runtime's `WireProtocolKind` variant for this marker.
    fn wire_protocol_kind_variant(self) -> proc_macro2::Ident {
        let name = match self {
            ProtocolRequestMarker::AwsJson => "AwsJson",
            ProtocolRequestMarker::RpcV2Cbor => "RpcV2Cbor",
            ProtocolRequestMarker::Query => "Query",
        };
        quote::format_ident!("{name}")
    }
}

/// A protocol, the trait that declares it, and the marker its requests carry.
///
/// `marker` is `None` for a protocol whose requests carry no marker: the REST
/// protocols route by a static method + URI table and mark nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Protocol {
    kind: ProtocolKind,
    trait_id: ProtocolTraitId,
    marker: Option<ProtocolRequestMarker>,
}

/// Every wire protocol a service can declare. A service's protocol list is the
/// entries whose trait its shape carries, in this order.
const PROTOCOLS: [Protocol; 7] = [
    Protocol {
        kind: ProtocolKind::AwsJson10,
        trait_id: ProtocolTraitId::AwsJson10,
        marker: Some(ProtocolRequestMarker::AwsJson),
    },
    Protocol {
        kind: ProtocolKind::AwsJson11,
        trait_id: ProtocolTraitId::AwsJson11,
        marker: Some(ProtocolRequestMarker::AwsJson),
    },
    Protocol {
        kind: ProtocolKind::RpcV2Cbor,
        trait_id: ProtocolTraitId::RpcV2Cbor,
        marker: Some(ProtocolRequestMarker::RpcV2Cbor),
    },
    Protocol {
        kind: ProtocolKind::AwsQuery,
        trait_id: ProtocolTraitId::AwsQuery,
        marker: Some(ProtocolRequestMarker::Query),
    },
    Protocol {
        kind: ProtocolKind::Ec2Query,
        trait_id: ProtocolTraitId::Ec2Query,
        marker: Some(ProtocolRequestMarker::Query),
    },
    Protocol {
        kind: ProtocolKind::RestXml,
        trait_id: ProtocolTraitId::RestXml,
        marker: None,
    },
    Protocol {
        kind: ProtocolKind::RestJson1,
        trait_id: ProtocolTraitId::RestJson1,
        marker: None,
    },
];

/// Refuse a protocol list that cannot be served per request. Selection reads the
/// request's marker, so two protocols behind one marker cannot be told apart, and
/// a protocol with no marker can only be recognised when it is the only one.
/// Returns the problem as a clause that continues "Service `X` ...".
fn check_protocols(protocols: &[Protocol]) -> Result<(), String> {
    if protocols.len() > 1 {
        if let Some(rest) = protocols.iter().find(|protocol| protocol.marker.is_none()) {
            let names: Vec<&str> = protocols
                .iter()
                .map(|protocol| protocol.kind.name())
                .collect();
            return Err(format!(
                "declares {} together: {} routes by method and URI and its requests carry no \
                 protocol marker, so a service that declares it must declare no other protocol",
                names.join(" and "),
                rest.kind
            ));
        }
    }
    for (index, protocol) in protocols.iter().enumerate() {
        if let Some(earlier) = protocols[..index]
            .iter()
            .find(|earlier| earlier.marker == protocol.marker)
        {
            return Err(format!(
                "declares both {} and {}: a request names its protocol by the same marker for \
                 both, so it cannot choose between them",
                earlier.kind, protocol.kind
            ));
        }
    }
    Ok(())
}

fn smithy_service_impl(
    attr: TokenStream,
    item_struct: syn::ItemStruct,
) -> syn::Result<TokenStream> {
    let attr_list = match darling::ast::NestedMeta::parse_meta_list(attr.into()) {
        Ok(v) => v,
        Err(e) => {
            return Err(syn::Error::new(
                e.span(),
                "Failed to parse attribute arguments",
            ))
        }
    };

    let opts = SmithyServiceOpts::from_list(&attr_list)
        .map_err(|e| syn::Error::new(proc_macro2::Span::call_site(), e.to_string()))?;

    let errors_path: syn::Path = match opts.errors.clone() {
        Some(p) => p,
        None => syn::parse_quote! { ::smithy_server_runtime::errors },
    };

    // The author's struct ident is deliberately ignored: the generated carrier
    // is always the fixed-name `ServiceDefinition`. Only the author's visibility is
    // carried onto it.
    let struct_vis = &item_struct.vis;

    let model_path = opts.model.as_deref().ok_or_else(|| {
        syn::Error::new_spanned(
            &item_struct,
            "no Smithy model path set: pass `model = \"...\"` to `#[smithy_service(...)]`",
        )
    })?;
    let model = load_smithy_model(model_path)?;

    let service_shape = model.get(&opts.service).ok_or_else(|| {
        syn::Error::new_spanned(
            &item_struct,
            shape_not_found_message("Service shape", &opts.service, &model),
        )
    })?;

    // Auto-detect the wire protocol from the service shape's traits.
    let service_traits = service_shape.get("traits").and_then(|t| t.as_object());
    let has_trait = |id: &str| service_traits.is_some_and(|t| t.contains_key(id));

    // The XML namespace the restXml codec stamps on the root element and on the
    // error envelopes comes from the SERVICE's `@xmlNamespace` trait — S3 says
    // `http://s3.amazonaws.com/doc/2006-03-01/`, Route 53 says
    // `https://route53.amazonaws.com/doc/2013-04-01/`. A service with no
    // `@xmlNamespace` gets none, which is what Smithy specifies (and what the
    // codec emits when `namespace` is `None`).
    let xml_namespace: Option<String> = service_traits
        .and_then(|t| t.get("smithy.api#xmlNamespace"))
        .and_then(|ns| ns.get("uri"))
        .and_then(|u| u.as_str())
        .map(str::to_owned);

    // restXml's `noErrorWrapping` member drops the `<ErrorResponse>` wrapper
    // from error responses — a service that sets it reads the error `<Code>`
    // off a bare `<Error>` root. Absent means wrapped (the Smithy default),
    // which every other restXml service expects. Read the same way as
    // `xml_namespace`, off the service's `aws.protocols#restXml` trait object.
    let no_error_wrapping: bool = service_traits
        .and_then(|t| t.get("aws.protocols#restXml"))
        .and_then(|t| t.get("noErrorWrapping"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // The service's SigV4 signing name, carried into the generated service's
    // `ServiceIdentity` (see the service assembly near the end of this
    // function). A signing name has two honest model sources: the service's
    // `aws.auth#sigv4` `name`, then the legacy `aws.api#service`
    // `endpointPrefix`. The signing name is NOT derivable from the shape id, so
    // a model that declares neither has no signing name, and the generated
    // service stores the empty string rather than a name synthesized from the
    // shape id. An empty signing name is honest: the generated `can_handle`
    // compares it against the request's resolved route key, which is never
    // empty for a real request, so a service with no declared signing name
    // claims no request instead of silently claiming requests under a fabricated
    // name. The empty string is the pre-live default; a model that needs the
    // generated service to route must declare `aws.auth#sigv4` or an
    // `endpointPrefix`.
    let signing_name: String = derive_signing_name(service_traits, || Ok(String::default()))?;
    // Normalize ALL three sources identically to what `resolve_route_key`
    // returns (trimmed + ASCII-lowercased). The generated `Service::can_handle`
    // compares this stored signing name against that resolved route key, so a
    // sigv4 `name`/`endpointPrefix` carrying uppercase or surrounding whitespace
    // would otherwise never match and `can_handle` would silently return false.
    let signing_name = signing_name.trim().to_ascii_lowercase();
    let service_shape_id: String = opts.service.clone();

    // Every protocol the model declares. The list is the service's protocol set:
    // codecs, error envelopes and per-request selection are all built from it,
    // and no entry outranks another. A service that declares none is refused.
    let protocols: Vec<Protocol> = PROTOCOLS
        .into_iter()
        .filter(|protocol| has_trait(protocol.trait_id.shape_id()))
        .collect();
    if protocols.is_empty() {
        return Err(syn::Error::new_spanned(
            &item_struct,
            format!(
                "Service '{}' has no recognised protocol trait (expected `aws.protocols#awsJson1_0`, `aws.protocols#awsJson1_1`, `smithy.protocols#rpcv2Cbor`, `aws.protocols#restJson1`, `aws.protocols#awsQuery`, `aws.protocols#ec2Query`, or `aws.protocols#restXml`)",
                opts.service
            ),
        ));
    }
    check_protocols(&protocols).map_err(|problem| {
        syn::Error::new_spanned(
            &item_struct,
            format!("Service '{}' {problem}", opts.service),
        )
    })?;

    // Both REST protocols share URI-based routing and the HTTP-binding layer;
    // they differ only in body codec (XML vs JSON). `is_rest` gates all the
    // routing/bridge machinery that is common to both. A REST protocol is always
    // the service's only one (`check_protocols`).
    //
    // rpcv2Cbor is also routed by request URI (`POST /service/{Service}/operation/{Op}`),
    // but dynamically, so it needs no route table. The awsJson family routes by
    // the `x-amz-target` header. awsQuery and ec2Query are NOT URI-routed either:
    // they route by the `Action` field of the form-urlencoded body, which the
    // protocol's `routing_key` reads directly, the same non-URI flow the awsJson
    // family uses.
    let is_rest = protocols.iter().any(|protocol| protocol.marker.is_none());

    // THE one place a protocol's response content-type is named.
    //
    // Only `PreDispatchProtocol::Json` reads it, and only restJson1, awsJson1_0
    // and awsJson1_1 select that variant (`envelope_of`): restXml, the query
    // family and rpcv2Cbor each have their own envelope variant. rpcv2Cbor has no
    // entry: it always stamps `application/cbor` itself, and labelling CBOR bytes
    // with any text type is parse-fatal for clients.
    //
    // awsJson1_1 must answer `application/x-amz-json-1.1`, not awsJson1_0's
    // type. The same per-protocol
    // values also exist in `aws_smithy_schema_server::rpc::{AwsJson10, AwsJson11}`,
    // so the two are pinned together by
    // `protocol_content_type_matches_the_server_protocol_constructors`.
    let content_type_of = |kind: ProtocolKind| -> Option<&'static str> {
        match kind {
            ProtocolKind::RestXml => Some("application/xml"),
            ProtocolKind::RestJson1 => Some("application/json"),
            ProtocolKind::AwsQuery | ProtocolKind::Ec2Query => Some("text/xml"),
            ProtocolKind::AwsJson10 => Some("application/x-amz-json-1.0"),
            ProtocolKind::AwsJson11 => Some("application/x-amz-json-1.1"),
            ProtocolKind::RpcV2Cbor => None,
        }
    };

    // Enumerate every operation the service binds — its own top-level
    // `operations` list UNIONED with everything reachable through its `resources`
    // tree (lifecycle bindings + operations/collectionOperations, recursing into
    // nested resources). Services like Lambda and Bedrock bind most or all of
    // their ops to resources, so enumerating only `service.operations` would
    // leave those ops unwired.
    let operation_targets = collect_operation_targets(service_shape, &model);

    // `operations = [...]` selects which of those the builder/dispatch actually
    // wires — useful when only a subset is implemented. Wiring every op a service
    // declares would require the consumer crate to define
    // `<Op>Input`/`<Op>Output`/`<Op>Error` for each, rarely what a
    // partial-fidelity emulator wants.
    let op_filter: HashSet<String> = opts.operations.iter().map(|s| s.value()).collect();

    // Whether the service shape carries an `operations` array at all (a
    // present-empty `operations: []` counts). This discriminates a deliberate
    // do-nothing service (valid — what the scaffold emits for a fresh project)
    // from a misconfigured/wrong `service` shape that declares no operation
    // binding whatsoever.
    let declares_operations_array = service_shape
        .get("operations")
        .and_then(|o| o.as_array())
        .is_some();

    let diagnostics =
        diagnose_op_targets(&operation_targets, &op_filter, declares_operations_array);

    // Restore the error the pre-resource-traversal code raised when a service
    // shape had no `operations` array: with the resource-tree union that check
    // was accidentally dropped, so a typo'd/wrong `service` shape now compiles
    // but wires nothing and only surfaces later as a "method not found" on the
    // builder. Resource-only services (Bedrock etc.) have a non-empty union and
    // a present-empty `operations: []` is still allowed — only a service that
    // binds nothing AND declares no `operations` array trips this.
    if diagnostics.no_op_binding {
        return Err(syn::Error::new_spanned(
            &item_struct,
            format!(
                "service `{}` declares no `operations` array and binds no operations through \
                 its `resources` tree. This is almost always a misconfigured `service = \"...\"` \
                 shape id or a malformed model. Point `#[smithy_service]` at a service that \
                 declares operations (directly, via a — possibly empty — `operations` list, or \
                 via resource lifecycle/operations bindings).",
                opts.service
            ),
        ));
    }

    // `operations(\"Foo\")` entries that matched no discovered operation wire
    // nothing: the builder gets no `foo(...)` method and the dispatch table no
    // arm, so a caller hits a compile error at `builder.foo(...)` or a runtime
    // "unknown operation" far from the declaration. Surface it here as a
    // compile-time warning attached to the emitted code (a `#[deprecated]` shim
    // is the stable-Rust way to raise a `warning:` from a proc-macro without a
    // nightly diagnostic API or an extra dependency).
    let unmatched_filter_warning: proc_macro2::TokenStream =
        if diagnostics.unmatched_filter.is_empty() {
            quote! {}
        } else {
            let listed = diagnostics
                .unmatched_filter
                .iter()
                .map(|f| format!("`{f}`"))
                .collect::<Vec<_>>()
                .join(", ");
            let note = format!(
                "#[smithy_service] operations(...) on service `{}` lists {} which matched no \
             operation discovered on the service (top-level `operations` \u{222a} the resource \
             tree); the entry wires nothing — check for a typo or a missing resource binding.",
                opts.service, listed
            );
            quote! {
                const _: () = {
                    #[deprecated(note = #note)]
                    struct SmithyServiceUnmatchedOperationFilter;
                    // Constructing the deprecated marker raises a compile-time
                    // `warning:` attributed to the `#[smithy_service]` declaration;
                    // this const block is discarded and nothing runs at runtime.
                    let _ = SmithyServiceUnmatchedOperationFilter;
                };
            }
        };

    let mut route_entries: Vec<proc_macro2::TokenStream> = Vec::new();

    // The generated `ServiceDefinition` and the per-operation `ErasedOperation`
    // impls, collected per operation and assembled near the end of this
    // function.
    //
    //  * `op_items`     — one `impl ErasedOperation for {Op}<Eng>` per op, on the
    //                     AUTHOR op struct.
    //  * `table_inits`  — one `routing_table.add_operation("{Op}", Box::new(..))`
    //                     statement per op, run in `ServiceDefinition::new` to build
    //                     the owned `OperationRoutingTable`. The operations live
    //                     in that table now, not as named fields on the entry, so
    //                     there is no per-op struct field or field init.
    let mut op_items: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut table_inits: Vec<proc_macro2::TokenStream> = Vec::new();

    let types_path = &opts.types;

    for op_target in &operation_targets {
        let op_target = op_target.as_str();

        let op_name = op_target
            .split('#')
            .nth(1)
            .ok_or_else(|| syn::Error::new_spanned(&item_struct, "Invalid operation shape ID"))?;

        if !op_filter.contains(op_name) {
            continue;
        }

        let op_shape = match model.get(op_target) {
            Some(s) => s,
            None => continue,
        };

        // An output member targeting a `@streaming` union makes this an
        // event-stream operation: dispatch frames that member rather than
        // serializing the output as a plain awsJson body.
        let streaming_output_field = op_shape
            .get("output")
            .and_then(|o| o.get("target"))
            .and_then(|t| t.as_str())
            .and_then(|out_id| model.get(out_id))
            .and_then(|out| out.get("members"))
            .and_then(|m| m.as_object())
            .and_then(|members| {
                members.iter().find_map(|(member_name, member)| {
                    let target = member.get("target")?.as_str()?;
                    codegen::is_event_stream_union(target, &model)
                        .then(|| make_field_ident(&to_snake_case(member_name)))
                })
            });

        let doc = op_shape
            .get("traits")
            .and_then(|t| t.get("smithy.api#documentation"))
            .and_then(|d| d.as_str())
            .unwrap_or("");

        let doc_attr = if !doc.is_empty() {
            quote! { #[doc = #doc] }
        } else {
            quote! {}
        };

        // The author owns `impl Operation for {Op}<Eng>` in `crate::ops::{op}`;
        // the macro emits no op struct, closure handler field, builder, or per-op
        // dispatch function. The only per-op items the macro emits are the
        // `ErasedOperation` shim (targeting the author struct) and the
        // `routing_table.add_operation("{Op}", Box::new(..))` statement run in
        // `ServiceDefinition::new` — both collected below.
        let _ = &doc_attr;

        // For REST protocols, collect HTTP trait info for the route table
        let http_trait = op_shape
            .get("traits")
            .and_then(|t| t.get("smithy.api#http"))
            .and_then(|h| h.as_object());
        let http_method = http_trait
            .and_then(|h| h.get("method"))
            .and_then(|m| m.as_str())
            .unwrap_or("POST");
        let http_uri = http_trait
            .and_then(|h| h.get("uri"))
            .and_then(|u| u.as_str())
            .unwrap_or("/");
        let _http_code = http_trait
            .and_then(|h| h.get("code"))
            .and_then(|c| c.as_u64())
            .unwrap_or(200) as u16;

        // The REST protocols (restXml + restJson1) use a static route table
        // (method + URI → operation). rpcv2Cbor is URI-routed too, but routes
        // dynamically via `routing_key_from_path`
        // (`/service/{S}/operation/{Op}`) and needs no route entries; the awsJson
        // family routes by the `x-amz-target` header. `is_rest` holds when the
        // service declares a REST protocol, so rpcv2Cbor is correctly excluded here.
        if is_rest {
            // Every `@required` member bound to a header or a query parameter is
            // a routing discriminator: REST services overload one method + URI
            // template across several operations (S3's four `PUT
            // /{Bucket}/{Key+}` writes) and differ only by an `x-id` literal
            // that AWS SDKs do not send. Handing the router the model's
            // required-binding set lets it reject a candidate whose required
            // header/query is absent, so `PutObject` is reachable without
            // `x-id`. `@httpLabel` is excluded: the path match already
            // guarantees it.
            let required_bindings = required_http_bindings(op_shape, &model);
            let route = quote! {
                ::aws_smithy_schema_server::Route::new(#http_method, #http_uri, #op_name)
            };
            route_entries.push(if required_bindings.is_empty() {
                route
            } else {
                quote! {
                    #route.with_required(&[#(#required_bindings),*])
                }
            });
        }

        // Per-op codegen. The macro emits no op struct, closure handler, or per-op
        // dispatch function. It emits ONLY:
        //   * the `ErasedOperation` shim, targeting the AUTHOR op struct
        //     `crate::ops::{op}::{Op}<__Eng>` (the author owns `impl Operation`),
        //   * the `routing_table.add_operation("{Op}", Box::new({Op}::new(engine.clone())))`
        //     statement run in `ServiceDefinition::new` — no closure, no builder bridge.
        //
        // The author struct is the bare operation name `{Op}` in the snake-case
        // op module under `crate::ops`. `#[path]`-globbed re-exports in
        // `ops/mod.rs` make `crate::ops::{snake}::{Op}` resolvable.
        let operation_module = make_field_ident(&to_snake_case(op_name));
        let operation_ty = quote::format_ident!("{}", op_name);
        let operation_path = quote! { crate::ops::#operation_module::#operation_ty };

        // Whether this operation's output is an event stream. A non-streaming
        // op runs the centralized deserialize -> validate -> handle ->
        // serialize sequence in `run_operation_http`. A `@streaming`-union output op
        // CANNOT be driven object-safely: its `serialize_as_event_stream<P>`
        // requires a `Sized` protocol, but `run` holds `&dyn ServerHttpProtocol`
        // (unsized), so the generated surface declines it with the framework's
        // "modeled but not served" signal (`UnknownOperation` -> 501 on
        // awsJson/REST), the same signal the system uses to decline event-stream
        // unions elsewhere.
        let is_streaming_output = streaming_output_field.is_some();

        // The body of the generated `ErasedOperation::run`. For a non-streaming
        // op it is a one-line shim into the centralized sequence: the shim is the
        // monomorphization site that recovers a concrete `O` (inside this
        // concrete `impl`, `Self = {Op}<__Eng>`), which the router cannot do
        // because it holds an erased `&dyn ErasedOperation`. The whole sequence --
        // deserialize, validate, handle, serialize, the `awsQueryCompatible`
        // header, and the framework-error tail that reaches the service's own
        // refusal wording via `svc.override_refusal` -- lives in the vended
        // `run_operation_http`, so nothing of it is emitted per operation. The shim
        // passes the envelope of the protocol the request is served in
        // (`svc.definition().wire_protocol(req)`), and the sequence reads the codec
        // from the same place. For a streaming op the body
        // declines, rendered through the same per-service pre-dispatch envelope
        // the sequence uses, so the two never diverge.
        let run_body = if is_streaming_output {
            quote! {
                let _ = (self, ctx);
                ::smithy_server_runtime::protocol_error_response_with(
                    svc.definition().wire_protocol(req).envelope(),
                    ::aws_smithy_schema_server::ProtocolError::UnknownOperation(
                        #op_name.to_string(),
                    ),
                    ::std::option::Option::None,
                )
            }
        } else {
            quote! {
                ::smithy_server_runtime::run_operation_http(
                    self,
                    svc,
                    req,
                    ctx,
                    svc.definition().wire_protocol(req).envelope(),
                )
                .await
            }
        };

        let auth_schemes = codegen::operation_auth_schemes(op_shape, service_shape);
        // The per-operation `ErasedOperation` impl, on the AUTHOR op struct.
        // This impl is a local impl in the service crate (the macro expands
        // there), so naming `crate::ops::{op}::{Op}` satisfies the orphan rule.
        // The author provides `impl Operation for {Op}<__Eng>`; this shim is the
        // monomorphization site `run_operation_http` needs.
        op_items.push(quote! {
            impl<__Eng: ::std::marker::Send + ::std::marker::Sync + 'static> ::smithy_server_runtime::ErasedOperation for #operation_path<__Eng> {
                fn run<'r>(
                    &'r self,
                    svc: &'r dyn ::smithy_server_runtime::Service,
                    req: &'r ::aws_smithy_runtime_api::http::Request,
                    ctx: &'r ::smithy_server_runtime::OperationContext<'r>,
                ) -> ::smithy_server_runtime::BoxFuture<'r, ::aws_smithy_runtime_api::http::Response> {
                    ::std::boxed::Box::pin(async move {
                        #run_body
                    })
                }
                fn auth_schemes(&self) -> ::std::option::Option<&'static [&'static str]> {
                    ::std::option::Option::Some(&[#(#auth_schemes),*])
                }
            }
        });

        // One `add_operation` statement per op, run in `ServiceDefinition::new` to
        // populate the owned `OperationRoutingTable`. The key is the model's
        // operation name (`#op_name`), the same `'static` literal the generated
        // lookup matched against before the table existed; the value is the
        // author op struct constructed from the shared engine
        // (`{Op}::new(engine.clone())`), boxed as a `dyn ErasedOperation`. No
        // per-op struct field and no field init: the operation lives in the
        // table, not on the entry.
        table_inits.push(quote! {
            routing_table.add_operation(
                #op_name,
                ::std::boxed::Box::new(#operation_path::new(engine.clone()))
                    as ::std::boxed::Box<dyn ::smithy_server_runtime::ErasedOperation>,
            );
        });
    }

    // One codec per protocol. Built after the operation loop because a REST
    // codec carries the route table the loop collected.
    let protocol_ctor_of = |kind: ProtocolKind| -> proc_macro2::TokenStream {
        let with_namespace = match &xml_namespace {
            Some(ns) => quote! { .with_namespace(#ns) },
            None => quote! {},
        };
        match kind {
            ProtocolKind::AwsJson10 => quote! { ::aws_smithy_schema_server::rpc::AwsJson10() },
            ProtocolKind::AwsJson11 => quote! { ::aws_smithy_schema_server::rpc::AwsJson11() },
            ProtocolKind::RpcV2Cbor => quote! { ::aws_smithy_schema_server::rpc::Rpcv2Cbor() },
            ProtocolKind::AwsQuery => {
                quote! { ::aws_smithy_schema_server::QueryServerProtocol::aws_query()#with_namespace }
            }
            ProtocolKind::Ec2Query => {
                quote! { ::aws_smithy_schema_server::QueryServerProtocol::ec2_query()#with_namespace }
            }
            ProtocolKind::RestXml => {
                // Only emit the builder call when the model sets the flag: the
                // codec's default is the wrapped error envelope.
                let with_no_wrap = if no_error_wrapping {
                    quote! { .with_no_error_wrapping(true) }
                } else {
                    quote! {}
                };
                quote! {
                    ::aws_smithy_schema_server::RestXmlServerProtocol::new(&[
                        #(#route_entries),*
                    ])#with_namespace #with_no_wrap
                }
            }
            ProtocolKind::RestJson1 => {
                quote! {
                    ::aws_smithy_schema_server::RestJsonServerProtocol::new(&[
                        #(#route_entries),*
                    ])
                }
            }
        }
    };

    // One error format per protocol: the envelope a framework-level failure
    // (validation, an unrouted operation, a malformed body) is rendered in.
    // Pre-dispatch protocol-error rendering lives in the runtime crate
    // (`smithy_server_runtime::pre_dispatch`), so it is compiled once for the
    // whole workspace; the macro emits only the protocol selector, whose only
    // per-service inputs are the protocol and (restXml only) the
    // `@xmlNamespace`. See that module's doc comment for the per-protocol
    // status/code asymmetries. ec2Query selects the ec2 shape of the query
    // error envelope.
    let envelope_of = |kind: ProtocolKind| -> proc_macro2::TokenStream {
        match kind {
            ProtocolKind::RestXml => {
                let xmlns = match &xml_namespace {
                    Some(ns) => quote! { Some(#ns) },
                    None => quote! { None },
                };
                quote! {
                    ::smithy_server_runtime::PreDispatchProtocol::RestXml {
                        xml_namespace: #xmlns,
                        no_error_wrapping: #no_error_wrapping,
                    }
                }
            }
            ProtocolKind::RpcV2Cbor => {
                quote! { ::smithy_server_runtime::PreDispatchProtocol::RpcV2Cbor }
            }
            ProtocolKind::AwsQuery => {
                quote! { ::smithy_server_runtime::PreDispatchProtocol::Query { ec2: false } }
            }
            ProtocolKind::Ec2Query => {
                quote! { ::smithy_server_runtime::PreDispatchProtocol::Query { ec2: true } }
            }
            // The awsJson family and restJson1 share one envelope, parameterised
            // by the protocol's content type; only restJson1 resolves an error
            // shape from `x-amzn-errortype`.
            ProtocolKind::AwsJson10 | ProtocolKind::AwsJson11 | ProtocolKind::RestJson1 => {
                let content_type = content_type_of(kind)
                    .expect("every JSON protocol has an entry in the content-type table");
                let errortype_header = kind == ProtocolKind::RestJson1;
                quote! {
                    ::smithy_server_runtime::PreDispatchProtocol::Json {
                        content_type: #content_type,
                        errortype_header: #errortype_header,
                    }
                }
            }
        }
    };

    // The marker a request names the protocol by, as the argument of
    // `DeclaredProtocol::new`.
    let marker_of = |marker: Option<ProtocolRequestMarker>| -> proc_macro2::TokenStream {
        match marker {
            Some(marker) => {
                let variant = marker.wire_protocol_kind_variant();
                quote! { ::std::option::Option::Some(::smithy_server_runtime::WireProtocolKind::#variant) }
            }
            None => quote! { ::std::option::Option::None },
        }
    };

    // Every declared protocol, each built by the same three functions.
    let declared_protocols: Vec<proc_macro2::TokenStream> = protocols
        .iter()
        .map(|protocol| {
            let marker = marker_of(protocol.marker);
            let codec = protocol_ctor_of(protocol.kind);
            let envelope = envelope_of(protocol.kind);
            quote! {
                ::smithy_server_runtime::DeclaredProtocol::new(
                    #marker,
                    ::std::boxed::Box::new(#codec),
                    #envelope,
                )
            }
        })
        .collect();

    // ========================================================================
    // Generated-service-entry assembly.
    //
    // One generated `ServiceDefinition` per emulated service, generic over the engine
    // `__Eng` so the macro names no concrete engine type (no crate dependency,
    // no vendored-manifest parity change). The entry carries the model-derived
    // half of a service — its identity, its wire protocol, and its owned
    // `OperationRoutingTable` — and nothing else. The behavioral half (the
    // `impl Service`: `can_handle`, the override seams, the framework-error
    // renderers) is NOT emitted here: it is hand-editable and scaffolded once
    // into the service crate's `service.rs`, which composes this entry. That is
    // the whole point of the split — the model-derived wiring stays generated
    // and drift-free, while the behavior an author tunes lives in a file the
    // regenerator never overwrites.
    //
    // The entry's inherent `new(engine)` builds the identity and protocol as
    // before and populates the routing table by constructing each author op from
    // the shared engine (`{Op}::new(engine.clone())`) and adding it under the
    // model's operation name — no closures and no builder.
    //
    // The type name is a fixed `ServiceDefinition`, independent of the author's struct
    // ident: the author declares the attribute on any unit struct, which the
    // macro consumes without re-emitting, and every service crate exposes the one
    // `ServiceDefinition` type disambiguated by its crate path. The scaffolded
    // `EmulatedService<__Eng>` wraps it as its `entry` field.
    //
    // A zero-operation service still gets a valid `ServiceDefinition`: an empty
    // routing table that answers `None` for every key. Its scaffolded
    // `can_handle` matches the request's resolved route key against this
    // service's signing name, so the generated services compose under
    // `MultiServiceRouter` without a consumer writing a predicate.
    let service_entry_name = quote::format_ident!("ServiceDefinition");
    let generate_service_and_operations = quote! {
        #struct_vis struct #service_entry_name<__Eng: ::std::marker::Send + ::std::marker::Sync + 'static> {
            identity: ::smithy_server_runtime::ServiceIdentity,
            protocols: ::smithy_server_runtime::DeclaredProtocols,
            routing_table: ::smithy_server_runtime::OperationRoutingTable,
            // Keeps `__Eng` used even for a zero-operation service, whose routing
            // table names no `__Eng`. `fn() -> __Eng` so the marker never adds a
            // drop or auto-trait constraint beyond the `Send + Sync` bound.
            __engine_marker: ::core::marker::PhantomData<fn() -> __Eng>,
        }

        impl<__Eng: ::std::marker::Send + ::std::marker::Sync + 'static> #service_entry_name<__Eng> {
            /// Build this service entry from the shared engine. The engine is
            /// threaded to each operation's constructor (the engine-as-constructor
            /// seam) as the routing table is populated, so engine access is
            /// guaranteed by the time any request runs; the operation's behaviour
            /// lives in the service author's `impl Operation`.
            pub fn new(
                engine: ::std::sync::Arc<__Eng>,
            ) -> Self {
                let _ = &engine;
                let mut routing_table = ::smithy_server_runtime::OperationRoutingTable::new();
                #(#table_inits)*
                #service_entry_name {
                    identity: ::smithy_server_runtime::ServiceIdentity::new(
                        #service_shape_id,
                        #signing_name,
                    ),
                    protocols: ::smithy_server_runtime::DeclaredProtocols::new(
                        ::std::vec![#(#declared_protocols),*],
                    ),
                    routing_table,
                    __engine_marker: ::core::marker::PhantomData,
                }
            }

            /// This service's vended identity (shape id + signing name). Reached
            /// through the `ErasedServiceDefinition` the scaffolded
            /// `Service::definition` hands back.
            pub fn identity(&self) -> &::smithy_server_runtime::ServiceIdentity {
                &self.identity
            }

            /// The legacy request-independent codec; see
            /// `smithy_server_runtime::ErasedServiceDefinition::protocol` for
            /// which codec it is, what to call instead, and why it is not
            /// marked deprecated.
            pub fn protocol(&self) -> &dyn ::smithy_server_runtime::ServerHttpProtocol {
                self.protocols.first_declared()
            }

            /// This service's model-derived operation routing table, built once
            /// in `new` and returned by reference (never rebuilt per call).
            /// Reached through the `ErasedServiceDefinition` the scaffolded
            /// `Service::definition` hands back.
            pub fn routing_table(&self) -> &::smithy_server_runtime::OperationRoutingTable {
                &self.routing_table
            }

            /// Render the framework-level routing failure in THIS service's own
            /// protocol envelope. The default the orchestrator renders when a
            /// service does not override it via `Service::override_routing_error`;
            /// reached through the `ErasedServiceDefinition`. This is model-derived (the
            /// per-protocol pre-dispatch
            /// declaration is baked at codegen), not an author seam: it recovers
            /// the protocol's own routing failure (unknown or absent key) and
            /// renders it through the SAME pre-dispatch path a matched operation
            /// uses for a framework error, so an unmatched op and a
            /// matched-but-failing op cannot answer in different envelopes. A
            /// routing key that resolves but matches no modeled operation is the
            /// framework's `UnknownOperation` signal, mapped per protocol
            /// (query -> 400 InvalidAction text/xml, awsJson -> 404,
            /// rpcv2Cbor -> 404, rest -> 501).
            pub fn routing_error_response(
                &self,
                req: &::aws_smithy_runtime_api::http::Request,
            ) -> ::aws_smithy_runtime_api::http::Response {
                self.wire_protocol(req).routing_error_response(req)
            }

            /// The protocol `req` is served in: its codec and error envelope. A
            /// service that declares one protocol serves every request in it; one
            /// that declares several serves the protocol the request's headers
            /// name, and refuses a request that names none of them (see
            /// `smithy_server_runtime::wire_protocol`). Reached through the
            /// `ErasedServiceDefinition`; the router and every operation read it,
            /// so each step of a request uses one protocol.
            pub fn wire_protocol(
                &self,
                req: &::aws_smithy_runtime_api::http::Request,
            ) -> ::smithy_server_runtime::WireProtocol<'_> {
                self.protocols.select(req)
            }

            /// Render an arbitrary framework-level `ProtocolError` for `req` in
            /// the envelope of the protocol `req` is served in. The default the orchestrator
            /// renders when a service does not override it via
            /// `Service::override_framework_error`; reached through the
            /// `ErasedServiceDefinition`. Model-derived,
            /// not an author seam: a transport-level failure the router detects up
            /// front (e.g. a malformed `aws-chunked` body, stripped once before any
            /// protocol reads it) is rendered through the SAME envelope
            /// `routing_error_response` uses for `req`, so a
            /// transport-level refusal and a routing refusal cannot answer in
            /// different envelopes.
            pub fn framework_error_response(
                &self,
                error: ::aws_smithy_schema_server::ProtocolError,
                req: &::aws_smithy_runtime_api::http::Request,
            ) -> ::aws_smithy_runtime_api::http::Response {
                self.wire_protocol(req).framework_error_response(error)
            }

            /// `framework_error_response` worded as the service chose through
            /// `Service::override_refusal`, in the envelope of the protocol `req`
            /// is served in. Reached through the `ErasedServiceDefinition`.
            pub fn refusal_response(
                &self,
                error: ::aws_smithy_schema_server::ProtocolError,
                chosen: ::std::option::Option<::smithy_server_runtime::Refused>,
                req: &::aws_smithy_runtime_api::http::Request,
            ) -> ::aws_smithy_runtime_api::http::Response {
                self.wire_protocol(req).refusal_response(error, chosen)
            }
        }

        // The engine-erased view the router reads this entry through. The
        // scaffolded `Service::definition` hands back `&self.entry` as a
        // `&dyn ErasedServiceDefinition`, so the router reads identity, wire
        // protocol, routing table, and the protocol-correct error renderers off a
        // `&dyn Service` without naming `__Eng`. Each method delegates to the
        // inherent method above — the inherent form stays the single home of the
        // body so the entry is also usable by its concrete type.
        impl<__Eng: ::std::marker::Send + ::std::marker::Sync + 'static>
            ::smithy_server_runtime::ErasedServiceDefinition for #service_entry_name<__Eng>
        {
            fn identity(&self) -> &::smithy_server_runtime::ServiceIdentity {
                #service_entry_name::identity(self)
            }

            fn protocol(&self) -> &dyn ::smithy_server_runtime::ServerHttpProtocol {
                #service_entry_name::protocol(self)
            }

            fn routing_table(&self) -> &::smithy_server_runtime::OperationRoutingTable {
                #service_entry_name::routing_table(self)
            }

            fn routing_error_response(
                &self,
                req: &::aws_smithy_runtime_api::http::Request,
            ) -> ::aws_smithy_runtime_api::http::Response {
                #service_entry_name::routing_error_response(self, req)
            }

            fn framework_error_response(
                &self,
                error: ::aws_smithy_schema_server::ProtocolError,
                req: &::aws_smithy_runtime_api::http::Request,
            ) -> ::aws_smithy_runtime_api::http::Response {
                #service_entry_name::framework_error_response(self, error, req)
            }

            fn refusal_response(
                &self,
                error: ::aws_smithy_schema_server::ProtocolError,
                chosen: ::std::option::Option<::smithy_server_runtime::Refused>,
                req: &::aws_smithy_runtime_api::http::Request,
            ) -> ::aws_smithy_runtime_api::http::Response {
                #service_entry_name::refusal_response(self, error, chosen, req)
            }

            fn wire_protocol(
                &self,
                req: &::aws_smithy_runtime_api::http::Request,
            ) -> ::smithy_server_runtime::WireProtocol<'_> {
                #service_entry_name::wire_protocol(self, req)
            }
        }

        // The per-operation `ErasedOperation` impls on the author op structs.
        #(#op_items)*
    };

    let expanded = quote! {
        #unmatched_filter_warning

        // The generated `ServiceDefinition<__Eng>` (identity + protocol + owned
        // routing table) with its inherent `new` constructor, and the
        // per-operation `ErasedOperation` impls. The author's `#[smithy_service]`
        // unit struct is consumed by this attribute macro and is NOT re-emitted;
        // the behavioral `impl Service` is scaffolded into the service crate's
        // editable `service.rs`, which wraps this entry.
        #generate_service_and_operations
    };

    Ok(TokenStream::from(expanded))
}

// =============================================================================
// Tests: diagnostics surface one clear root error, not a cascade
// =============================================================================

#[cfg(test)]
mod diagnostics_tests {
    use super::*;
    use std::sync::Mutex;

    fn err(msg: &str) -> syn::Error {
        syn::Error::new(proc_macro2::Span::call_site(), msg)
    }

    fn item() -> proc_macro2::TokenStream {
        quote::quote! { pub struct FooInput { pub bar: String } }
    }

    /// The dominant real-world cascade: one bad model observed by every
    /// `#[smithy(...)]` in a module. All invocations report the SAME root
    /// cause, so exactly ONE full `compile_error!` must surface — the rest are
    /// suppressed to just the re-emitted item.
    #[test]
    fn identical_root_cause_emits_single_diagnostic() {
        let seen: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
        let e = "Failed to load Smithy model: no such file";

        let first = render_codegen_error(err(e), Some(item()), &seen).to_string();
        let second = render_codegen_error(err(e), Some(item()), &seen).to_string();
        let third = render_codegen_error(err(e), Some(item()), &seen).to_string();

        assert_eq!(
            [&first, &second, &third]
                .iter()
                .filter(|o| o.contains("compile_error"))
                .count(),
            1,
            "exactly one full diagnostic should surface for a repeated root cause"
        );
        assert!(first.contains("compile_error"), "first occurrence surfaces");
        assert!(!second.contains("compile_error"), "duplicate suppressed");
        assert!(!third.contains("compile_error"), "duplicate suppressed");
        // The item is preserved in every case so its type still resolves and we
        // don't trade the cascade for a "cannot find type" cascade.
        for o in [&first, &second, &third] {
            assert!(o.contains("FooInput"), "item re-emitted every time");
        }
    }

    /// Distinct root causes are independent — each surfaces once.
    #[test]
    fn distinct_root_causes_each_surface() {
        let seen: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
        let a = render_codegen_error(err("Shape 'A' not found in Smithy model"), None, &seen)
            .to_string();
        let b = render_codegen_error(err("Shape 'B' not found in Smithy model"), None, &seen)
            .to_string();
        assert!(a.contains("compile_error") && a.contains('A'));
        assert!(b.contains("compile_error") && b.contains('B'));
    }

    /// The actionable diagnostic must come BEFORE the re-emitted item so it is
    /// not buried under downstream noise.
    #[test]
    fn diagnostic_is_emitted_before_item() {
        let out = build_error_output(&err("root cause here"), Some(&item()), true).to_string();
        let ce = out.find("compile_error").expect("compile_error present");
        let it = out.find("FooInput").expect("item present");
        assert!(ce < it, "compile_error must precede the re-emitted item");
    }

    #[test]
    fn suppressed_output_has_no_compile_error_but_keeps_item() {
        let out = build_error_output(&err("dup"), Some(&item()), false).to_string();
        assert!(!out.contains("compile_error"));
        assert!(out.contains("FooInput"));
    }

    #[test]
    fn levenshtein_basic() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("", "abc"), 3);
    }

    #[test]
    fn suggests_typo_shape_id() {
        let keys = [
            "com.example#PutItemInput".to_string(),
            "com.example#GetItemInput".to_string(),
            "com.example#DeleteItemInput".to_string(),
        ];
        // A one-character typo should surface the intended shape.
        let got = nearest_shape_ids("com.example#PutItemInpu", keys.iter());
        assert!(
            got.iter().any(|s| s == "com.example#PutItemInput"),
            "expected typo suggestion, got {:?}",
            got
        );
    }

    #[test]
    fn suggests_wrong_namespace_by_local_name() {
        let keys = ["com.example#PutItemInput".to_string()];
        // Right local name, wrong namespace — the single most common mistake.
        let got = nearest_shape_ids("com.wrong#PutItemInput", keys.iter());
        assert_eq!(got, vec!["com.example#PutItemInput".to_string()]);
    }

    #[test]
    fn unrelated_id_yields_no_suggestion() {
        let keys = ["com.example#PutItemInput".to_string()];
        let got = nearest_shape_ids("zzz#TotallyUnrelatedName", keys.iter());
        assert!(
            got.is_empty(),
            "should not suggest unrelated shapes: {:?}",
            got
        );
    }

    #[test]
    fn shape_not_found_message_is_actionable() {
        let mut model: HashMap<String, serde_json::Value> = HashMap::new();
        model.insert(
            "com.example#PutItemInput".to_string(),
            serde_json::json!({"type": "structure"}),
        );
        let msg = shape_not_found_message("Shape", "com.wrong#PutItemInput", &model);
        assert!(msg.contains("not found in Smithy model"));
        assert!(msg.contains("help: did you mean"));
        assert!(msg.contains("com.example#PutItemInput"));
        assert!(msg.contains("note:"), "always includes the model-path hint");
    }
}

#[cfg(test)]
mod signing_name_tests {
    use super::*;

    fn traits(json: &str) -> serde_json::Map<String, serde_json::Value> {
        match serde_json::from_str(json).unwrap() {
            serde_json::Value::Object(m) => m,
            _ => panic!("expected a JSON object"),
        }
    }

    // The server (`smithy_service`) and client (`smithy_client`) both derive the
    // signing name through this one helper, so for any model they MUST agree.
    // The invariant only has teeth on the tier-2 fallback: with no
    // `aws.auth#sigv4.name`, a divergent key read would silently break scope
    // matching, since the client signs under its name and the server claims the
    // request by comparing its name against the resolved route key.
    #[test]
    fn server_and_client_agree_on_tier2_fallback() {
        // No `aws.auth#sigv4.name`; only the tier-2 `endpointPrefix`.
        let t = traits(
            r#"{
                "aws.auth#sigv4": {},
                "aws.api#service": { "endpointPrefix": "dynamodb", "arnNamespace": "dynamodb" }
            }"#,
        );
        // The server's call: empty-string on_missing.
        let server = derive_signing_name(Some(&t), || Ok(String::default())).unwrap();
        // The client's call: hard-Err on_missing (never reached here).
        let client = derive_signing_name(Some(&t), || {
            Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "unreachable",
            ))
        })
        .unwrap();
        assert_eq!(
            server, client,
            "server and client must derive the same name"
        );
        assert_eq!(server, "dynamodb");
    }

    // `endpointPrefix` is preferred over `arnNamespace`: when both are present
    // the helper reads `endpointPrefix` (tier-2), reaching `arnNamespace`
    // (tier-3) only in its absence. Both front doors read the SAME chain, so
    // the client can never sign under a name the server does not claim by.
    #[test]
    fn endpoint_prefix_is_preferred_over_arn_namespace() {
        let t = traits(
            r#"{
                "aws.auth#sigv4": {},
                "aws.api#service": { "endpointPrefix": "execute-api", "arnNamespace": "apigateway" }
            }"#,
        );
        let name = derive_signing_name(Some(&t), || Ok(String::default())).unwrap();
        assert_eq!(name, "execute-api", "endpointPrefix wins over arnNamespace");
    }

    // tier-3: with no `aws.auth#sigv4.name` and no `endpointPrefix`, the helper
    // falls back to `arnNamespace`, so a model that declares only `arnNamespace`
    // still resolves a name (the same one on both front doors) instead of
    // hitting `on_missing`.
    #[test]
    fn tier3_falls_back_to_arn_namespace() {
        let t = traits(
            r#"{
                "aws.auth#sigv4": {},
                "aws.api#service": { "arnNamespace": "apigateway" }
            }"#,
        );
        let server = derive_signing_name(Some(&t), || Ok(String::default())).unwrap();
        let client = derive_signing_name(Some(&t), || {
            Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "unreachable",
            ))
        })
        .unwrap();
        assert_eq!(
            server, client,
            "server and client agree on the tier-3 fallback"
        );
        assert_eq!(server, "apigateway", "tier-3 fallback reads arnNamespace");
    }

    // tier-1 (`aws.auth#sigv4.name`) still wins over the tier-2 `endpointPrefix`.
    #[test]
    fn tier1_sigv4_name_takes_precedence() {
        let t = traits(
            r#"{
                "aws.auth#sigv4": { "name": "s3" },
                "aws.api#service": { "endpointPrefix": "s3-outposts" }
            }"#,
        );
        let name = derive_signing_name(Some(&t), || Ok(String::default())).unwrap();
        assert_eq!(name, "s3");
    }

    // When neither source is present, each front door keeps its own policy:
    // the server returns the empty string, the client returns an Err.
    #[test]
    fn on_missing_policy_is_caller_supplied() {
        let t = traits(r#"{ "aws.auth#sigv4": {} }"#);
        let server = derive_signing_name(Some(&t), || Ok(String::default())).unwrap();
        assert_eq!(server, "", "server fabricates no name");
        let client = derive_signing_name(Some(&t), || {
            Err(syn::Error::new(proc_macro2::Span::call_site(), "no name"))
        });
        assert!(client.is_err(), "client errors when no name can be derived");
    }
}

#[cfg(test)]
mod resource_traversal_tests {
    use super::*;

    fn model_map(json: &str) -> HashMap<String, serde_json::Value> {
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        value
            .get("shapes")
            .and_then(|s| s.as_object())
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    fn service_shape(model: &HashMap<String, serde_json::Value>) -> serde_json::Value {
        model
            .values()
            .find(|s| s.get("type").and_then(|t| t.as_str()) == Some("service"))
            .cloned()
            .unwrap()
    }

    /// A Lambda/Bedrock-shaped model: a top-level op, a resource tree with
    /// lifecycle + operations + collectionOperations, a nested resource, and an
    /// op reachable via BOTH a top-level entry and a resource `list` slot (must
    /// de-dup).
    fn resource_model() -> &'static str {
        r#"{
          "smithy": "2.0",
          "shapes": {
            "com.amazonaws.lambda#Svc": {
              "type": "service",
              "operations": [
                { "target": "com.amazonaws.lambda#GetAccountSettings" },
                { "target": "com.amazonaws.lambda#ListFunctions" }
              ],
              "resources": [ { "target": "com.amazonaws.lambda#FunctionResource" } ]
            },
            "com.amazonaws.lambda#FunctionResource": {
              "type": "resource",
              "create": { "target": "com.amazonaws.lambda#CreateFunction" },
              "read": { "target": "com.amazonaws.lambda#GetFunction" },
              "update": { "target": "com.amazonaws.lambda#UpdateFunctionConfiguration" },
              "delete": { "target": "com.amazonaws.lambda#DeleteFunction" },
              "list": { "target": "com.amazonaws.lambda#ListFunctions" },
              "operations": [ { "target": "com.amazonaws.lambda#Invoke" } ],
              "collectionOperations": [ { "target": "com.amazonaws.lambda#ListFunctionsByCodeSigningConfig" } ],
              "resources": [ { "target": "com.amazonaws.lambda#AliasResource" } ]
            },
            "com.amazonaws.lambda#AliasResource": {
              "type": "resource",
              "create": { "target": "com.amazonaws.lambda#CreateAlias" },
              "delete": { "target": "com.amazonaws.lambda#DeleteAlias" }
            }
          }
        }"#
    }

    #[test]
    fn unions_service_ops_with_the_resource_tree_deterministically() {
        let model = model_map(resource_model());
        let svc = service_shape(&model);
        let targets = collect_operation_targets(&svc, &model);
        assert_eq!(
            targets,
            vec![
                "com.amazonaws.lambda#GetAccountSettings".to_string(),
                "com.amazonaws.lambda#ListFunctions".to_string(),
                "com.amazonaws.lambda#CreateFunction".to_string(),
                "com.amazonaws.lambda#GetFunction".to_string(),
                "com.amazonaws.lambda#UpdateFunctionConfiguration".to_string(),
                "com.amazonaws.lambda#DeleteFunction".to_string(),
                "com.amazonaws.lambda#Invoke".to_string(),
                "com.amazonaws.lambda#ListFunctionsByCodeSigningConfig".to_string(),
                "com.amazonaws.lambda#CreateAlias".to_string(),
                "com.amazonaws.lambda#DeleteAlias".to_string(),
            ]
        );
    }

    #[test]
    fn service_operations_only_is_unchanged() {
        // No `resources`: enumeration equals the declared `service.operations`,
        // same order — the byte-identical guarantee for sqs/dynamodb/iam/etc.
        let model = model_map(
            r#"{
              "smithy": "2.0",
              "shapes": {
                "com.amazonaws.sqs#Svc": {
                  "type": "service",
                  "operations": [
                    { "target": "com.amazonaws.sqs#CreateQueue" },
                    { "target": "com.amazonaws.sqs#DeleteQueue" },
                    { "target": "com.amazonaws.sqs#GetQueueUrl" }
                  ]
                }
              }
            }"#,
        );
        let svc = service_shape(&model);
        assert_eq!(
            collect_operation_targets(&svc, &model),
            vec![
                "com.amazonaws.sqs#CreateQueue".to_string(),
                "com.amazonaws.sqs#DeleteQueue".to_string(),
                "com.amazonaws.sqs#GetQueueUrl".to_string(),
            ]
        );
    }

    #[test]
    fn resource_only_service_with_no_top_level_operations_still_enumerates() {
        // Bedrock-shaped: the service declares NO top-level `operations`, only a
        // resource tree. Previously this errored ("no operations array"); now it
        // enumerates the resource-bound ops.
        let model = model_map(
            r#"{
              "smithy": "2.0",
              "shapes": {
                "com.example#Svc": {
                  "type": "service",
                  "resources": [ { "target": "com.example#Thing" } ]
                },
                "com.example#Thing": {
                  "type": "resource",
                  "read": { "target": "com.example#GetThing" },
                  "list": { "target": "com.example#ListThings" }
                }
              }
            }"#,
        );
        let svc = service_shape(&model);
        assert_eq!(
            collect_operation_targets(&svc, &model),
            vec![
                "com.example#GetThing".to_string(),
                "com.example#ListThings".to_string(),
            ]
        );
    }

    #[test]
    fn empty_service_yields_no_targets() {
        let model = model_map(
            r#"{ "smithy": "2.0", "shapes": { "com.example#Svc": { "type": "service" } } }"#,
        );
        let svc = service_shape(&model);
        assert!(collect_operation_targets(&svc, &model).is_empty());
    }

    /// A resource tree nested FOUR levels deep, mirroring real models
    /// (CodeCatalyst, MediaPackageV2) whose resources nest well past depth 2.
    /// A `put` lifecycle slot is exercised at depth 2 so a future reorder or
    /// removal of `put` from [`RESOURCE_LIFECYCLE_SLOTS`] is caught (G3).
    fn deeply_nested_resource_model() -> &'static str {
        r#"{
          "smithy": "2.0",
          "shapes": {
            "com.example#Svc": {
              "type": "service",
              "operations": [ { "target": "com.example#TopOp" } ],
              "resources": [ { "target": "com.example#R1" } ]
            },
            "com.example#R1": {
              "type": "resource",
              "read": { "target": "com.example#GetR1" },
              "resources": [ { "target": "com.example#R2" } ]
            },
            "com.example#R2": {
              "type": "resource",
              "put": { "target": "com.example#PutR2" },
              "resources": [ { "target": "com.example#R3" } ]
            },
            "com.example#R3": {
              "type": "resource",
              "read": { "target": "com.example#GetR3" },
              "resources": [ { "target": "com.example#R4" } ]
            },
            "com.example#R4": {
              "type": "resource",
              "read": { "target": "com.example#GetR4" },
              "operations": [ { "target": "com.example#PokeR4" } ]
            }
          }
        }"#
    }

    #[test]
    fn discovers_ops_nested_four_levels_deep() {
        // Recursion must reach ops bound at depth 3 (R3.GetR3) and depth 4
        // (R4.GetR4 / R4.PokeR4), not just the shallow depth-1/2 cases.
        let model = model_map(deeply_nested_resource_model());
        let svc = service_shape(&model);
        let targets = collect_operation_targets(&svc, &model);
        assert_eq!(
            targets,
            vec![
                "com.example#TopOp".to_string(),
                "com.example#GetR1".to_string(),  // depth 1
                "com.example#PutR2".to_string(),  // depth 2 (put slot)
                "com.example#GetR3".to_string(),  // depth 3
                "com.example#GetR4".to_string(),  // depth 4 (lifecycle)
                "com.example#PokeR4".to_string(), // depth 4 (operations)
            ]
        );
    }

    #[test]
    fn discovers_put_lifecycle_bound_op() {
        // Guards RESOURCE_LIFECYCLE_SLOTS containing "put": 41 real services use
        // the `put` slot, so removing/reordering it out would silently drop ops.
        let model = model_map(deeply_nested_resource_model());
        let svc = service_shape(&model);
        assert!(
            collect_operation_targets(&svc, &model).contains(&"com.example#PutR2".to_string()),
            "the `put` lifecycle-bound op must be discovered"
        );
    }

    #[test]
    fn diagnose_flags_a_missing_operations_binding_as_a_hard_error() {
        // G1: a service that declares NO `operations` array and binds nothing via
        // resources trips the hard `compile_error!` (misconfigured service/model).
        let diag = diagnose_op_targets(&[], &HashSet::new(), false);
        assert!(
            diag.no_op_binding,
            "no operations array + empty union must be a hard error"
        );
        assert!(diag.unmatched_filter.is_empty());
    }

    #[test]
    fn diagnose_allows_a_present_empty_operations_list() {
        // A present-but-empty `operations: []` is a valid do-nothing service (what
        // the scaffold emits for a fresh project; see the rpcv2-cbor/aws-query e2e
        // crates). It must NOT trip the hard error even though the union is empty.
        let diag = diagnose_op_targets(&[], &HashSet::new(), true);
        assert!(
            !diag.no_op_binding,
            "a present-empty operations list is a valid zero-op service"
        );
    }

    #[test]
    fn diagnose_reports_operations_filter_entries_that_match_nothing() {
        // G4: `operations("GetThing", "Typo")` where only GetThing is discovered
        // must report `Typo` as unmatched (→ compile-time warning), while the
        // discovered entry is silent.
        let targets = vec![
            "com.example#GetThing".to_string(),
            "com.example#ListThings".to_string(),
        ];
        let filter: HashSet<String> = ["GetThing".to_string(), "Typo".to_string()]
            .into_iter()
            .collect();
        let diag = diagnose_op_targets(&targets, &filter, true);
        assert!(!diag.no_op_binding);
        assert_eq!(diag.unmatched_filter, vec!["Typo".to_string()]);
    }

    #[test]
    fn diagnose_is_silent_when_every_filter_entry_matches() {
        let targets = vec!["com.example#GetThing".to_string()];
        let filter: HashSet<String> = ["GetThing".to_string()].into_iter().collect();
        let diag = diagnose_op_targets(&targets, &filter, true);
        assert!(diag.unmatched_filter.is_empty());
    }
}

#[cfg(test)]
mod required_http_binding_tests {
    use super::*;

    fn model_map(json: &str) -> HashMap<String, serde_json::Value> {
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        value
            .get("shapes")
            .and_then(|s| s.as_object())
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// The four-way `PUT /{Bucket}/{Key+}` collision from the S3 model, reduced to
    /// just the members that decide it.
    const S3_WRITES: &str = r#"{"shapes":{
      "com.example#PutObject": {"type":"operation","input":{"target":"com.example#PutObjectRequest"}},
      "com.example#PutObjectRequest": {"type":"structure","members":{
        "Bucket":{"target":"smithy.api#String","traits":{"smithy.api#httpLabel":{},"smithy.api#required":{}}},
        "Key":{"target":"smithy.api#String","traits":{"smithy.api#httpLabel":{},"smithy.api#required":{}}},
        "ContentType":{"target":"smithy.api#String","traits":{"smithy.api#httpHeader":"content-type"}}
      }},
      "com.example#CopyObject": {"type":"operation","input":{"target":"com.example#CopyObjectRequest"}},
      "com.example#CopyObjectRequest": {"type":"structure","members":{
        "Bucket":{"target":"smithy.api#String","traits":{"smithy.api#httpLabel":{},"smithy.api#required":{}}},
        "CopySource":{"target":"smithy.api#String","traits":{"smithy.api#httpHeader":"x-amz-copy-source","smithy.api#required":{}}}
      }},
      "com.example#UploadPart": {"type":"operation","input":{"target":"com.example#UploadPartRequest"}},
      "com.example#UploadPartRequest": {"type":"structure","members":{
        "PartNumber":{"target":"smithy.api#Integer","traits":{"smithy.api#httpQuery":"partNumber","smithy.api#required":{}}},
        "UploadId":{"target":"smithy.api#String","traits":{"smithy.api#httpQuery":"uploadId","smithy.api#required":{}}}
      }},
      "com.example#NoInput": {"type":"operation"},
      "com.example#EmptyInput": {"type":"operation","input":{"target":"com.example#Empty"}},
      "com.example#Empty": {"type":"structure"},
      "com.example#Unnamed": {"type":"operation","input":{"target":"com.example#UnnamedRequest"}},
      "com.example#UnnamedRequest": {"type":"structure","members":{
        "Token":{"target":"smithy.api#String","traits":{"smithy.api#httpQuery":{},"smithy.api#required":{}}}
      }},
      "com.example#Dangling": {"type":"operation","input":{"target":"com.example#Missing"}}
    }}"#;

    fn bindings_for(op: &str) -> Vec<String> {
        let model = model_map(S3_WRITES);
        let shape = model.get(op).expect("operation in fixture").clone();
        required_http_bindings(&shape, &model)
            .iter()
            .map(|ts| ts.to_string().replace(' ', ""))
            .collect()
    }

    #[test]
    fn required_header_binding_is_collected() {
        let bindings = bindings_for("com.example#CopyObject");
        assert_eq!(bindings.len(), 1, "one required header binding");
        assert!(
            bindings[0].contains("RequiredBinding::Header(\"x-amz-copy-source\")"),
            "got {bindings:?}"
        );
    }

    #[test]
    fn required_query_bindings_are_collected() {
        let bindings = bindings_for("com.example#UploadPart");
        assert_eq!(bindings.len(), 2);
        assert!(bindings
            .iter()
            .any(|b| b.contains("RequiredBinding::Query(\"partNumber\")")));
        assert!(bindings
            .iter()
            .any(|b| b.contains("RequiredBinding::Query(\"uploadId\")")));
    }

    /// Negative: `@httpLabel` members are excluded (the path match already implies
    /// them) and an OPTIONAL header binding carries no obligation. `PutObject`
    /// therefore has no discriminating bindings at all — which is exactly why it
    /// must win a plain object write.
    #[test]
    fn labels_and_optional_bindings_are_excluded() {
        assert!(
            bindings_for("com.example#PutObject").is_empty(),
            "only required header/query members qualify"
        );
    }

    /// Negative: a `@httpQuery` whose trait value is not a string falls back to
    /// the member name rather than being dropped.
    #[test]
    fn binding_without_an_explicit_wire_name_uses_the_member_name() {
        let bindings = bindings_for("com.example#Unnamed");
        assert_eq!(bindings.len(), 1);
        assert!(
            bindings[0].contains("RequiredBinding::Query(\"Token\")"),
            "got {bindings:?}"
        );
    }

    /// Negative: operations with no input, an input with no members, or an input
    /// target missing from the model all yield no bindings instead of panicking.
    #[test]
    fn operations_without_usable_input_yield_no_bindings() {
        assert!(bindings_for("com.example#NoInput").is_empty());
        assert!(bindings_for("com.example#EmptyInput").is_empty());
        assert!(bindings_for("com.example#Dangling").is_empty());
    }
}

#[cfg(test)]
mod xml_list_layout_tests {
    use super::*;

    fn model_map(json: &str) -> HashMap<String, serde_json::Value> {
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        value
            .get("shapes")
            .and_then(|s| s.as_object())
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Both real S3 arrangements plus the wrapped baseline.
    const MODEL: &str = r#"{"shapes":{
      "com.example#ShapeFlattened": {"type":"list","member":{"target":"smithy.api#String"},
        "traits":{"smithy.api#xmlFlattened":{}}},
      "com.example#WrappedNamed": {"type":"list",
        "member":{"target":"smithy.api#String","traits":{"smithy.api#xmlName":"Object"}}},
      "com.example#WrappedPlain": {"type":"list","member":{"target":"smithy.api#String"}}
    }}"#;

    fn layout(member_json: &str) -> (bool, Option<String>) {
        let model = model_map(MODEL);
        let member: serde_json::Value = serde_json::from_str(member_json).unwrap();
        let (flattened, item) = xml_list_layout(&member, &model);
        (flattened, item.map(str::to_owned))
    }

    /// S3's actual arrangement: `"flattened": true` on the LIST SHAPE, with the
    /// element named by `@xmlName` on the STRUCTURE MEMBER (`Delete.Objects`).
    /// Missing this is what made a shape-level flattened list unparseable.
    #[test]
    fn flattened_on_the_list_shape_is_detected() {
        let (flattened, item) = layout(
            r#"{"target":"com.example#ShapeFlattened","traits":{"smithy.api#xmlName":"Object"}}"#,
        );
        assert!(
            flattened,
            "@xmlFlattened on the list shape must be honoured"
        );
        assert_eq!(item, None, "this list's member carries no @xmlName");
    }

    /// The other legal placement: `@xmlFlattened` directly on the member.
    #[test]
    fn flattened_on_the_member_is_detected() {
        let (flattened, _) = layout(
            r#"{"target":"com.example#WrappedPlain","traits":{"smithy.api#xmlFlattened":{}}}"#,
        );
        assert!(flattened);
    }

    /// Negative: no `@xmlFlattened` anywhere means the wrapped layout, and the
    /// item name comes from the list member's `@xmlName`.
    #[test]
    fn wrapped_list_reports_its_item_name_and_is_not_flattened() {
        let (flattened, item) = layout(r#"{"target":"com.example#WrappedNamed"}"#);
        assert!(!flattened);
        assert_eq!(item.as_deref(), Some("Object"));
    }

    /// Negative: a wrapped list whose member has no `@xmlName` reports no item
    /// name (the codec then defaults to `member`).
    #[test]
    fn wrapped_list_without_member_xml_name_reports_none() {
        let (flattened, item) = layout(r#"{"target":"com.example#WrappedPlain"}"#);
        assert!(!flattened);
        assert_eq!(item, None);
    }

    /// Negative: a target missing from the model yields the wrapped default
    /// rather than panicking.
    #[test]
    fn unresolvable_target_yields_wrapped_default() {
        let (flattened, item) = layout(r#"{"target":"com.example#Missing"}"#);
        assert!(!flattened);
        assert_eq!(item, None);
    }
}

#[cfg(test)]
mod xml_map_layout_tests {
    use super::*;

    fn model_map(json: &str) -> HashMap<String, serde_json::Value> {
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        value
            .get("shapes")
            .and_then(|s| s.as_object())
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    const MODEL: &str = r#"{"shapes":{
      "com.example#ShapeFlattened": {"type":"map",
        "key":{"target":"smithy.api#String"},"value":{"target":"smithy.api#String"},
        "traits":{"smithy.api#xmlFlattened":{}}},
      "com.example#Wrapped": {"type":"map",
        "key":{"target":"smithy.api#String"},"value":{"target":"smithy.api#String"}}
    }}"#;

    fn layout(member_json: &str) -> bool {
        let model = model_map(MODEL);
        let member: serde_json::Value = serde_json::from_str(member_json).unwrap();
        xml_map_layout(&member, &model)
    }

    /// `@xmlFlattened` on the map shape itself is honoured.
    #[test]
    fn flattened_on_the_map_shape_is_detected() {
        assert!(layout(r#"{"target":"com.example#ShapeFlattened"}"#));
    }

    /// The other legal placement: `@xmlFlattened` directly on the member.
    #[test]
    fn flattened_on_the_member_is_detected() {
        assert!(layout(
            r#"{"target":"com.example#Wrapped","traits":{"smithy.api#xmlFlattened":{}}}"#
        ));
    }

    /// Negative: no `@xmlFlattened` anywhere means the wrapped layout.
    #[test]
    fn wrapped_map_is_not_flattened() {
        assert!(!layout(r#"{"target":"com.example#Wrapped"}"#));
    }

    /// Negative: a target missing from the model yields the wrapped default
    /// rather than panicking.
    #[test]
    fn unresolvable_target_yields_wrapped_default() {
        assert!(!layout(r#"{"target":"com.example#Missing"}"#));
    }
}

// =============================================================================
// Tests: generated schema constants are collision-free, and a `@default`
// member's fallback never imposes a `Default` bound the field type cannot meet
// =============================================================================

#[cfg(test)]
mod default_and_schema_const_tests {
    use super::*;

    fn ts_default(json: &str) -> String {
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        default_member_fallback_expr(&codegen::ShapeKind::Timestamp, Some(&value)).to_string()
    }

    /// The `Exception` suffix is preserved, so the two spellings AWS ships side
    /// by side in one namespace (CloudWatch's `ResourceNotFound` and
    /// `ResourceNotFoundException`) produce distinct constants. Stripping the
    /// suffix collapsed them onto one name and the module failed with `E0428`.
    #[test]
    fn exception_suffix_does_not_collide_with_its_bare_twin() {
        let bare = error_name_to_schema_const("ResourceNotFound");
        let suffixed = error_name_to_schema_const("ResourceNotFoundException");
        assert_eq!(bare, "SCHEMA_RESOURCE_NOT_FOUND");
        assert_eq!(suffixed, "SCHEMA_RESOURCE_NOT_FOUND_EXCEPTION");
        assert_ne!(
            bare, suffixed,
            "an error shape and its `Exception`-suffixed twin must not share a constant"
        );
    }

    /// Every `@error` shape name in a real colliding service maps to a unique
    /// constant.
    #[test]
    fn all_error_names_in_a_colliding_service_map_uniquely() {
        let names = [
            "ResourceNotFound",
            "ResourceNotFoundException",
            "InvalidParameterValueException",
            "LimitExceededFault",
            "ConcurrentModificationException",
        ];
        let consts: HashSet<String> = names
            .iter()
            .map(|n| error_name_to_schema_const(n))
            .collect();
        assert_eq!(
            consts.len(),
            names.len(),
            "distinct error shapes must yield distinct constants: {consts:?}"
        );
    }

    /// A name that already lacks the suffix is unchanged by the fix.
    #[test]
    fn non_exception_names_are_unaffected() {
        assert_eq!(
            error_name_to_schema_const("LimitExceededFault"),
            "SCHEMA_LIMIT_EXCEEDED_FAULT"
        );
    }

    /// A `@default` timestamp materializes the model's whole-second default via
    /// the INTEGER constructor, and never emits `unwrap_or_default`, which would
    /// require `DateTime: Default`.
    ///
    /// The `!from_secs_f64` half matters: `from_secs_f64` contains `from_secs` as
    /// a substring, so asserting only the latter would still pass if a regression
    /// routed whole seconds through the fractional constructor.
    #[test]
    fn timestamp_default_uses_declared_whole_seconds() {
        let out = ts_default("0");
        assert!(
            out.contains("from_secs") && !out.contains("from_secs_f64") && out.contains('0'),
            "expected an explicit DateTime::from_secs(0), got {out}"
        );
        assert!(
            !out.contains("Default"),
            "a timestamp fallback must not rely on Default: {out}"
        );
    }

    /// A non-zero default is honoured rather than silently replaced by a zero
    /// value, and still takes the integer constructor.
    #[test]
    fn timestamp_default_honours_non_zero_value() {
        let out = ts_default("1700000000");
        assert!(
            out.contains("from_secs")
                && !out.contains("from_secs_f64")
                && out.contains("1700000000"),
            "expected the declared epoch to survive codegen, got {out}"
        );
    }

    /// A fractional default keeps its sub-second component.
    #[test]
    fn timestamp_default_supports_fractional_seconds() {
        let out = ts_default("1.5");
        assert!(
            out.contains("from_secs_f64"),
            "expected fractional constructor, got {out}"
        );
    }

    /// Negative: an absent default still yields a `DateTime` (the Smithy zero
    /// value for a timestamp), never a `Default`-bounded expression.
    #[test]
    fn timestamp_without_declared_default_falls_back_to_epoch_zero() {
        let out = default_member_fallback_expr(&codegen::ShapeKind::Timestamp, None).to_string();
        assert!(
            out.contains("from_secs") && !out.contains("from_secs_f64"),
            "got {out}"
        );
        assert!(!out.contains("Default"), "got {out}");
    }

    /// Negative: a default the macro cannot read as a number (Smithy also allows
    /// a date-time string) degrades to epoch zero rather than failing codegen.
    #[test]
    fn timestamp_with_unreadable_default_falls_back_to_epoch_zero() {
        let out = ts_default(r#""2020-01-01T00:00:00Z""#);
        assert!(
            out.contains("from_secs") && !out.contains("from_secs_f64"),
            "got {out}"
        );
        assert!(!out.contains("Default"), "got {out}");
    }

    /// Negative: a whole-seconds default too large for `i64` takes the
    /// fractional path rather than saturating through `as i64`, which would
    /// silently substitute `i64::MAX`.
    #[test]
    fn timestamp_default_out_of_i64_range_avoids_a_saturating_cast() {
        let out = ts_default("1e30");
        assert!(
            out.contains("from_secs_f64"),
            "an out-of-range default must not go through the i64 path: {out}"
        );
        assert!(
            !out.contains("9223372036854775807"),
            "must not saturate to i64::MAX: {out}"
        );
    }

    /// Negative, boundary: exactly 2^63. `i64::MAX` is 2^63 - 1 and is NOT
    /// representable in `f64` (it rounds up to 2^63), so a guard written against
    /// `i64::MAX as f64` would admit this value and then saturate. This pins the
    /// exclusive 2^63 bound.
    #[test]
    fn timestamp_default_at_two_pow_63_avoids_a_saturating_cast() {
        let out = ts_default("9223372036854775808");
        assert!(
            out.contains("from_secs_f64"),
            "2^63 must take the fractional path, not the i64 path: {out}"
        );
        assert!(
            !out.contains("9223372036854775807"),
            "must not saturate to i64::MAX: {out}"
        );
    }

    /// Positive, boundary: the largest whole second that IS representable in
    /// `i64` after the `f64` round-trip still takes the integer path, so the
    /// guard is not needlessly conservative.
    #[test]
    fn timestamp_default_just_below_two_pow_63_uses_the_integer_path() {
        // 2^62 is exactly representable in f64 and well inside i64.
        let out = ts_default("4611686018427387904");
        assert!(
            out.contains("from_secs") && !out.contains("from_secs_f64"),
            "2^62 should still use the integer constructor: {out}"
        );
    }

    /// Every other shape kind keeps `Default::default()`, so behaviour there is
    /// unchanged. This also pins the documented limitation: `default_value` is
    /// ignored for those kinds, so a non-zero declared default still resolves to
    /// the type's zero value.
    #[test]
    fn non_timestamp_kinds_still_use_default() {
        for kind in [
            codegen::ShapeKind::String,
            codegen::ShapeKind::Integer,
            codegen::ShapeKind::Boolean,
            codegen::ShapeKind::Blob,
            codegen::ShapeKind::Document,
        ] {
            let out = default_member_fallback_expr(&kind, None).to_string();
            assert!(
                out.contains("Default"),
                "{kind:?} should keep Default::default(), got {out}"
            );
        }
    }

    /// A declared default IS materialized now, for every kind that can carry one.
    ///
    /// `@default: 10` arriving as 0 is a wire-visible lie, and it becomes a false 400
    /// the moment the member is validated: every SDK paginator omits `maxResults`, so
    /// `@range(min: 1)` would reject the 0 on the FIRST page of a List call.
    #[test]
    fn a_declared_default_is_materialized() {
        let cases: Vec<(codegen::ShapeKind<'_>, &str, &str)> = vec![
            (codegen::ShapeKind::Integer, "5", "5"),
            (codegen::ShapeKind::Long, "100", "100"),
            (codegen::ShapeKind::Short, "7", "7"),
            (codegen::ShapeKind::Byte, "3", "3"),
            (codegen::ShapeKind::Boolean, "true", "true"),
            (codegen::ShapeKind::Double, "1.5", "1.5"),
            (codegen::ShapeKind::String, "\"abc\"", "abc"),
        ];
        for (kind, json, expected) in cases {
            let value: serde_json::Value = serde_json::from_str(json).unwrap();
            let out = default_member_fallback_expr(&kind, Some(&value)).to_string();
            assert!(
                out.contains(expected),
                "{kind:?} must materialize {expected}, got {out}"
            );
            assert!(
                !out.contains("Default"),
                "{kind:?} must not fall back to the type zero, got {out}"
            );
        }
    }

    /// A default that does not FIT its field falls back to the type zero rather than
    /// wrapping. `200 as i8` is `-56`, which would emit a confidently wrong value; a
    /// visible zero is the lesser evil, and this only fires on a model defect (Smithy
    /// requires a `@default` to be compatible with its own shape).
    #[test]
    fn an_out_of_range_default_falls_back_instead_of_wrapping() {
        for (kind, json) in [
            (codegen::ShapeKind::Byte, "200"),
            (codegen::ShapeKind::Byte, "-200"),
            (codegen::ShapeKind::Short, "70000"),
            (codegen::ShapeKind::Integer, "5000000000"),
        ] {
            let value: serde_json::Value = serde_json::from_str(json).unwrap();
            let out = default_member_fallback_expr(&kind, Some(&value)).to_string();
            assert!(
                out.contains("Default"),
                "{kind:?} default {json} does not fit and must fall back, got {out}"
            );
            assert!(!out.contains("56"), "must never wrap: {out}");
        }
    }

    /// A non-finite float must NOT reach `Literal::f*_suffixed`, which asserts finiteness
    /// and panics at codegen — failing the build of a whole crate rather than one
    /// operation. Smithy permits NaN/Infinity as a default, so a model can reach this.
    #[test]
    fn a_non_finite_float_default_does_not_panic_codegen() {
        for kind in [codegen::ShapeKind::Float, codegen::ShapeKind::Double] {
            for raw in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                let value = serde_json::Value::from(raw);
                // serde_json renders non-finite as null; either way this must not panic
                // and must not emit a literal.
                let out = default_member_fallback_expr(&kind, Some(&value)).to_string();
                assert!(out.contains("Default"), "{kind:?} {raw} -> {out}");
            }
        }
    }

    /// An enum-targeted default materializes the modeled wire string, exactly as a
    /// `String` does, because that is the field type generated for an `enum` shape.
    ///
    /// This is load-bearing rather than cosmetic: the type zero is `""`, which no
    /// enum's value set contains, so leaving the member to the zero turns a request
    /// that legally OMITS it into a 400 against the enum check.
    #[test]
    fn an_enum_default_materializes_the_modeled_value() {
        let value: serde_json::Value = serde_json::from_str("\"ACTIVE\"").unwrap();
        let out = default_member_fallback_expr(&codegen::ShapeKind::Enum, Some(&value)).to_string();
        assert!(out.contains("ACTIVE"), "got {out}");
        assert!(
            !out.contains("Default"),
            "must not fall back to the type zero: {out}"
        );
    }

    /// A non-string `@default` against an enum shape is a model defect, not something
    /// to guess at: it falls back to the type zero rather than emitting a literal of
    /// the wrong Rust type, which would not compile.
    #[test]
    fn a_non_string_enum_default_falls_back_to_the_zero() {
        let value: serde_json::Value = serde_json::from_str("7").unwrap();
        let out = default_member_fallback_expr(&codegen::ShapeKind::Enum, Some(&value)).to_string();
        assert!(out.contains("Default"), "got {out}");
    }

    /// `@default: null` means the member has NO default, and `@addedDefault` keeps it
    /// optional. Both satisfy a bare `contains_key`, which would produce a
    /// non-`Option`, zero-filled member that its own `@range` then rejects.
    #[test]
    fn a_null_or_added_default_is_not_a_usable_default() {
        let parse = |s: &str| -> serde_json::Map<String, serde_json::Value> {
            serde_json::from_str(s).unwrap()
        };

        let real = parse(r#"{"smithy.api#default": 10}"#);
        assert_eq!(
            usable_default(Some(&real)).and_then(|v| v.as_i64()),
            Some(10)
        );

        let null = parse(r#"{"smithy.api#default": null}"#);
        assert!(
            usable_default(Some(&null)).is_none(),
            "@default: null means NO default"
        );

        let added = parse(r#"{"smithy.api#default": 10, "smithy.api#addedDefault": {}}"#);
        assert!(
            usable_default(Some(&added)).is_none(),
            "@addedDefault keeps the member optional"
        );

        assert!(usable_default(Some(&parse("{}"))).is_none());
        assert!(usable_default(None).is_none());
    }
}

#[cfg(test)]
mod protocol_list_tests {
    use super::*;

    use ProtocolKind::{AwsJson10, AwsJson11, AwsQuery, Ec2Query, RestJson1, RestXml, RpcV2Cbor};

    /// The table entry for each of `kinds`, in order.
    fn protocols(kinds: &[ProtocolKind]) -> Vec<Protocol> {
        kinds
            .iter()
            .map(|kind| {
                *PROTOCOLS
                    .iter()
                    .find(|protocol| protocol.kind == *kind)
                    .unwrap_or_else(|| panic!("{kind} is in the protocol table"))
            })
            .collect()
    }

    fn accepted(kinds: &[ProtocolKind]) {
        assert_eq!(check_protocols(&protocols(kinds)), Ok(()), "{kinds:?}");
    }

    fn refused(kinds: &[ProtocolKind]) -> String {
        check_protocols(&protocols(kinds)).expect_err(&format!("{kinds:?} must be refused"))
    }

    #[test]
    fn every_protocol_is_accepted_alone() {
        for protocol in PROTOCOLS {
            assert_eq!(check_protocols(&[protocol]), Ok(()), "{protocol:?}");
        }
    }

    #[test]
    fn protocols_with_different_markers_are_accepted_together() {
        accepted(&[AwsJson10, RpcV2Cbor]);
        accepted(&[AwsJson11, RpcV2Cbor]);
        accepted(&[AwsJson10, RpcV2Cbor, AwsQuery]);
        accepted(&[AwsJson11, RpcV2Cbor, Ec2Query]);
    }

    #[test]
    fn two_json_versions_share_a_marker_and_are_refused() {
        let problem = refused(&[AwsJson10, AwsJson11]);
        assert!(
            problem.contains("awsJson1_0") && problem.contains("awsJson1_1"),
            "{problem}"
        );
    }

    #[test]
    fn two_query_flavours_share_a_marker_and_are_refused() {
        let problem = refused(&[AwsQuery, Ec2Query]);
        assert!(
            problem.contains("awsQuery") && problem.contains("ec2Query"),
            "{problem}"
        );
    }

    #[test]
    fn a_collision_is_found_among_other_protocols() {
        refused(&[AwsJson10, RpcV2Cbor, AwsJson11]);
    }

    #[test]
    fn a_rest_protocol_with_any_other_protocol_is_refused() {
        for rest in [RestJson1, RestXml] {
            for other in [AwsJson10, RpcV2Cbor, AwsQuery] {
                let problem = refused(&[other, rest]);
                assert!(
                    problem.contains(rest.name()) && problem.contains("no protocol marker"),
                    "{problem}"
                );
            }
        }
    }

    #[test]
    fn two_rest_protocols_are_refused() {
        refused(&[RestXml, RestJson1]);
    }

    #[test]
    fn the_protocol_table_lists_each_kind_once_with_its_trait() {
        let kinds: HashSet<ProtocolKind> = PROTOCOLS.iter().map(|protocol| protocol.kind).collect();
        assert_eq!(kinds.len(), PROTOCOLS.len());
        for protocol in PROTOCOLS {
            let shape_id = protocol.trait_id.shape_id();
            assert!(
                shape_id.ends_with(&format!("#{}", protocol.kind)),
                "{}: {shape_id}",
                protocol.kind
            );
        }
    }
}
