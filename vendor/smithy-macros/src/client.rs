// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! `#[smithy_client(...)]` — the client-side analogue of `#[smithy_service]`.
//!
//! Given a service shape ID, a module of operation input/output types, and a
//! list of operations, this generates a typed client struct with one async
//! method per operation. Each method reuses the operation's
//! `<Op>Input::SCHEMA` / `<Op>Output::SCHEMA` and the `SerializableStruct` /
//! `DeserializableStruct` impls the `#[smithy(...)]` macro already produced, and
//! drives the real SDK orchestrator through `amzn-smithy-client-runtime`.
//!
//! Everything protocol-specific is read from the model, exactly as the server
//! macro does it:
//!
//! - the wire protocol (awsJson1_0 / awsJson1_1) from the service's protocol
//!   trait;
//! - the SigV4 signing name from `aws.auth#sigv4`;
//! - the `x-amz-target` prefix from the service shape's local name
//!   (`DynamoDB_20120810`).
//!
//! The generated client is transport-agnostic: you hand it a
//! `smithy_client_runtime::Transport` (HTTP client + endpoint + credentials), so
//! the *same* client type talks to the in-process emulator or to real AWS.

use crate::{load_smithy_model, to_snake_case};
use darling::FromMeta;
use proc_macro::TokenStream;
use quote::quote;
use std::collections::HashSet;

#[derive(Debug, FromMeta)]
pub(crate) struct SmithyClientOpts {
    /// The Smithy shape ID of the service (e.g.
    /// `"com.amazonaws.dynamodb#DynamoDB_20120810"`).
    service: String,
    /// Path to the Rust module holding the operation input/output types.
    types: syn::Path,
    /// Workspace-relative path to the Smithy model JSON. Required.
    #[darling(default)]
    model: Option<String>,
    /// Operation names (without namespace) to generate methods for. Each named
    /// operation must have `<Op>Input` / `<Op>Output` types in `types`.
    #[darling(default)]
    operations: Vec<syn::LitStr>,
}

pub(crate) fn smithy_client_impl(
    attr: TokenStream,
    item_struct: syn::ItemStruct,
) -> syn::Result<TokenStream> {
    let attr_list = darling::ast::NestedMeta::parse_meta_list(attr.into())
        .map_err(|e| syn::Error::new(e.span(), "Failed to parse attribute arguments"))?;
    let opts = SmithyClientOpts::from_list(&attr_list)
        .map_err(|e| syn::Error::new(proc_macro2::Span::call_site(), e.to_string()))?;

    let struct_name = &item_struct.ident;
    let struct_vis = &item_struct.vis;
    let types_path = &opts.types;

    let model_path = opts.model.as_deref().ok_or_else(|| {
        syn::Error::new_spanned(
            &item_struct,
            "no Smithy model path set: pass `model = \"...\"` to `#[smithy_client(...)]`",
        )
    })?;
    let model = load_smithy_model(model_path)?;

    let service_shape = model.get(&opts.service).ok_or_else(|| {
        syn::Error::new_spanned(
            &item_struct,
            format!("Service shape '{}' not found in Smithy model", opts.service),
        )
    })?;

    // ---- protocol detection (mirrors smithy_service) ----
    let service_traits = service_shape.get("traits").and_then(|t| t.as_object());
    let has_trait = |id: &str| service_traits.is_some_and(|t| t.contains_key(id));
    let protocol_ctor: proc_macro2::TokenStream = if has_trait("aws.protocols#awsJson1_0") {
        quote! { ::smithy_client_runtime::AwsJsonProtocol::aws_json_1_0() }
    } else if has_trait("aws.protocols#awsJson1_1") {
        quote! { ::smithy_client_runtime::AwsJsonProtocol::aws_json_1_1() }
    } else {
        return Err(syn::Error::new_spanned(
            &item_struct,
            format!(
                "Service '{}' has no supported client protocol trait (expected `aws.protocols#awsJson1_0` or `aws.protocols#awsJson1_1`)",
                opts.service
            ),
        ));
    };

    // ---- auth scheme + signing name, derived from the model's auth traits ----
    //
    // The auth *scheme* is chosen by which auth trait the service declares
    // (today only `aws.auth#sigv4`, matching the awsJson RPC services this
    // client targets); its signing name comes from `aws.auth#sigv4.name`, else
    // the `aws.api#service` `endpointPrefix`, else its `arnNamespace` — the SAME
    // chain the server derives from, so the name the client signs under matches
    // the name the server claims the request by. Both are baked into the generated
    // `AuthScheme` value so the runtime never assumes SigV4 or a signing name.
    let sigv4 = service_traits.and_then(|t| t.get("aws.auth#sigv4"));
    if sigv4.is_none() {
        return Err(syn::Error::new_spanned(
            &item_struct,
            format!(
                "Service '{}' has no `aws.auth#sigv4` trait; only SigV4 auth is supported by `#[smithy_client]` today",
                opts.service
            ),
        ));
    }
    let signing_name = crate::derive_signing_name(service_traits, || {
        Err(syn::Error::new_spanned(
            &item_struct,
            format!(
                "Service '{}' has no `aws.auth#sigv4` name, `aws.api#service` endpointPrefix, or arnNamespace to derive the SigV4 signing name from",
                opts.service
            ),
        ))
    })?;
    // The model-derived auth scheme, emitted as an `AuthScheme` literal.
    let auth_ctor: proc_macro2::TokenStream = quote! {
        ::smithy_client_runtime::AuthScheme::SigV4 { signing_name: #signing_name }
    };

    // ---- x-amz-target prefix = service shape local name (e.g. DynamoDB_20120810) ----
    let target_prefix = opts
        .service
        .split('#')
        .nth(1)
        .ok_or_else(|| syn::Error::new_spanned(&item_struct, "Invalid service shape ID"))?
        .to_string();

    let operations = service_shape
        .get("operations")
        .and_then(|ops| ops.as_array())
        .ok_or_else(|| {
            syn::Error::new_spanned(&item_struct, "Service shape has no operations array")
        })?;

    let op_filter: HashSet<String> = opts.operations.iter().map(|s| s.value()).collect();

    let mut methods = Vec::new();

    for op in operations {
        let op_target = op
            .get("target")
            .and_then(|t| t.as_str())
            .ok_or_else(|| syn::Error::new_spanned(&item_struct, "Operation missing target"))?;
        let op_name = op_target
            .split('#')
            .nth(1)
            .ok_or_else(|| syn::Error::new_spanned(&item_struct, "Invalid operation shape ID"))?;

        if !op_filter.is_empty() && !op_filter.contains(op_name) {
            continue;
        }
        if model.get(op_target).is_none() {
            continue;
        }

        let method_name = quote::format_ident!("{}", to_snake_case(op_name));
        let input_type = quote::format_ident!("{}Input", op_name);
        let output_type = quote::format_ident!("{}Output", op_name);
        let target = format!("{target_prefix}.{op_name}");
        let op_name_lit = op_name;

        let doc = model
            .get(op_target)
            .and_then(|s| s.get("traits"))
            .and_then(|t| t.get("smithy.api#documentation"))
            .and_then(|d| d.as_str())
            .unwrap_or("");
        let doc_attr = if doc.is_empty() {
            quote! {}
        } else {
            quote! { #[doc = #doc] }
        };

        methods.push(quote! {
            #doc_attr
            pub async fn #method_name(
                &self,
                input: #types_path::#input_type,
            ) -> ::std::result::Result<
                #types_path::#output_type,
                ::smithy_client_runtime::MicroClientError,
            > {
                // The operation's static identity: everything derived from the
                // model, as a plain literal — no per-call allocation.
                let shape = ::smithy_client_runtime::OperationShape {
                    protocol: #protocol_ctor,
                    auth: #auth_ctor,
                    service_name: #signing_name,
                    operation_name: #op_name_lit,
                    target: #target,
                    input_schema: &#types_path::#input_type::SCHEMA,
                    output_schema: &#types_path::#output_type::SCHEMA,
                    _marker: ::std::marker::PhantomData,
                };
                ::smithy_client_runtime::Operation::new(shape, self.transport.clone())
                    .invoke(input)
                    .await
            }
        });
    }

    let signing_name_lit = &signing_name;

    let expanded = quote! {
        #[derive(Clone, Debug)]
        #struct_vis struct #struct_name {
            transport: ::smithy_client_runtime::Transport,
        }

        impl #struct_name {
            /// The SigV4 signing name for this service, as declared in the model.
            pub const SIGNING_NAME: &'static str = #signing_name_lit;

            /// Build a client over an explicit transport (HTTP client + endpoint
            /// + credentials). Use this to point at the in-process emulator or at a
            /// custom endpoint.
            pub fn new(transport: ::smithy_client_runtime::Transport) -> Self {
                Self { transport }
            }

            /// The transport this client uses.
            pub fn transport(&self) -> &::smithy_client_runtime::Transport {
                &self.transport
            }

            #(#methods)*
        }
    };

    Ok(TokenStream::from(expanded))
}
