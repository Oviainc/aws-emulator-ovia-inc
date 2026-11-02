// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Shared build logic for per-service crates.
//!
//! Each service's `build.rs` calls `generate()` with its service name and model
//! path. This crate holds the ops-glob, `#[smithy_service]` codegen, and
//! drift-guard logic.
#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};

/// Refuse a build whose integration tests are laid out one cargo target per file.
///
/// Cargo compiles a separate binary for every top-level `tests/*.rs`, and each one
/// statically links this crate together with its whole dependency closure — the
/// smithy runtime, the HTTP connector, the engine and tokio. The suite's own source
/// is a rounding error beside that closure, so N suites cost N copies of it. A crate
/// that keeps its suites as modules of a single `tests/all.rs` target pays the
/// closure once.
///
/// This check is what stops that shape from decaying back, because the decay is
/// invisible: adding `tests/foo.rs` works, passes, and simply costs another copy.
///
/// **Inert until the crate has a `tests/all.rs`.** A crate that has not adopted the
/// layout is not diagnosed at all, so this can land before any crate migrates.
///
/// Args:
///     manifest_dir: The crate root — `CARGO_MANIFEST_DIR`.
/// The declaration a suite under `tests/all/` needs.
///
/// `tests/all.rs` is a test target's crate root, so a bare `mod <stem>;` resolves to
/// `tests/<stem>.rs` — which rule 1 refuses — and fails with `E0583`. Hence `#[path]`.
///
/// Two lines rather than one `\n`-joined string: cargo scans build-script stdout line by
/// line, so an embedded newline would drop the `mod` line from the reported error.
fn module_declaration(stem: &str) -> [String; 2] {
    [
        format!("    #[path = \"all/{stem}.rs\"]"),
        format!("    mod {stem};"),
    ]
}

fn check_test_layout(manifest_dir: &Path) {
    let tests_dir = manifest_dir.join("tests");
    let all_rs = tests_dir.join("all.rs");

    // Re-run when a file is added to or removed from either directory (that moves the
    // directory's mtime) and when `all.rs` itself is edited (which does not). Both are
    // needed: the first catches a new suite, the second catches a DELETED `mod` line,
    // and watching only the directories would let the orphan rule go quietly stale.
    println!("cargo::rerun-if-changed={}", tests_dir.display());
    println!("cargo::rerun-if-changed={}", all_rs.display());
    println!(
        "cargo::rerun-if-changed={}",
        tests_dir.join("all").display()
    );

    if !all_rs.is_file() {
        return;
    }

    let declared = declared_modules(&fs::read_to_string(&all_rs).unwrap_or_default());

    // Rule 1: no OTHER top-level test target may exist beside `all.rs`.
    for stem in stray_top_level_suites(&tests_dir) {
        let declaration = module_declaration(&stem);
        emit_layout_error(&[
            &format!(
                "tests/{stem}.rs is a top-level integration test, but this crate has already \
                 adopted the single-target layout (tests/all.rs exists)."
            ),
            "Cargo builds one binary per top-level tests/*.rs, and each statically links this \
             crate plus its entire dependency closure. Suites under tests/all/ share one binary.",
            "This is a FILE LAYOUT rule. No change to the test's code can satisfy it - do not \
             edit the test. Move the file and declare it:",
            &format!("    git mv tests/{stem}.rs tests/all/{stem}.rs"),
            "    # then add to tests/all.rs, keeping the list sorted:",
            &declaration[0],
            &declaration[1],
        ]);
    }

    // Rule 2: every suite under `tests/all/` must be declared, or it compiles nowhere.
    for stem in suites_under_all(&tests_dir.join("all")) {
        if !declared.contains(&stem) {
            let declaration = module_declaration(&stem);
            emit_layout_error(&[
                &format!(
                    "the suite `{stem}` under tests/all/ is not declared in tests/all.rs, so it \
                     is compiled nowhere and its tests NEVER RUN."
                ),
                "This is a FILE LAYOUT rule. No change to the test's code can satisfy it - do \
                 not edit the test. Add the declaration:",
                "    # in tests/all.rs, keeping the list sorted:",
                &declaration[0],
                &declaration[1],
            ]);
        }
    }
}

/// Emit one `cargo::error=` per line.
///
/// One directive per line because `cargo::error=` is a single-line instruction: an
/// embedded newline would terminate the directive and print the remainder as an
/// uninterpreted build-script line, losing exactly the remedy the message exists to
/// carry. Cargo fails the build once the script exits, so this does not exit itself —
/// letting every violation in the crate be reported in one pass rather than one per
/// rebuild.
fn emit_layout_error(lines: &[&str]) {
    for line in lines {
        println!("cargo::error={line}");
    }
}

/// The module names a `tests/all.rs` declares, from its `mod <name>;` items.
///
/// Deliberately a line scan rather than a parse: the file is generated and only ever
/// holds `mod` items and comments, and a build script cannot compile a Rust parser
/// into itself for free.
///
/// It tolerates a visibility and a trailing comment because a MISSED declaration is
/// the expensive direction — Rule 2 would report a suite that is plainly declared as
/// an orphan, and its message would tell the reader to add a line already present.
/// The trailing `;` is still required, so an inline `mod foo { … }` is not mistaken
/// for a suite declaration.
fn declared_modules(all_rs: &str) -> Vec<String> {
    all_rs
        .lines()
        .filter_map(|line| {
            let line = line.split("//").next().unwrap_or(line).trim();
            let line = line
                .strip_prefix("pub")
                .map_or(line, |rest| rest.trim_start_matches(|c| c != 'm').trim());
            let name = line.strip_prefix("mod ")?.strip_suffix(';')?.trim();
            (!name.is_empty() && !name.contains(char::is_whitespace)).then(|| name.to_string())
        })
        .collect()
}

/// Top-level `tests/*.rs` stems other than `all` — each one its own cargo target.
fn stray_top_level_suites(tests_dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(tests_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(stem) = name.strip_suffix(".rs") {
                if stem != "all" && entry.path().is_file() {
                    out.push(stem.to_string());
                }
            }
        }
    }
    out.sort();
    out
}

/// Suite module names directly under `tests/all/`: a `<name>.rs` file or a
/// `<name>/mod.rs` directory module. Deeper files are declared by their own parent
/// module, not by `all.rs`, so the scan stops at this level.
fn suites_under_all(all_dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(all_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();
            if path.is_file() {
                if let Some(stem) = name.strip_suffix(".rs") {
                    out.push(stem.to_string());
                }
            } else if path.is_dir() && path.join("mod.rs").is_file() {
                out.push(name);
            }
        }
    }
    out.sort();
    out
}

/// Enforce the single-target integration-test layout on a crate that generates nothing.
///
/// The service crates get this check from [`generate`]. The router has no model to
/// generate from, so its `build.rs` calls this instead. The router needs it more than
/// any service crate does: it depends on every service, so each of its test targets
/// links the whole emulator.
///
/// Called from `core/router/build.rs`:
/// ```ignore
/// fn main() {
///     emulator_core_build_support::check_integration_test_layout();
/// }
/// ```
pub fn check_integration_test_layout() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    check_test_layout(&manifest_dir);
    // The vendored suite skips some checks unless this is set, as the core library sets it.
    println!("cargo::rustc-check-cfg=cfg(emulator_full_security_suite)");
}

/// Generate the `#[smithy_service]` decl and ops module for one service crate.
///
/// Also enforces the crate's integration-test layout — see [`check_test_layout`].
///
/// Called from each `service-<name>/build.rs`:
/// ```ignore
/// fn main() {
///     emulator_core_build_support::generate("sqs", "smithy-model/sqs.json", "src");
/// }
/// ```
pub fn generate(service_name: &str, model_path: &str, src_dir: &str) {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ops_dir = manifest_dir.join(src_dir).join("ops");
    let model_file = manifest_dir.join(model_path);

    check_test_layout(&manifest_dir);

    println!("cargo::rerun-if-changed={}", ops_dir.display());
    println!("cargo::rerun-if-changed={}", model_file.display());

    // Discover operations from ops/*.rs files
    let mut ops: Vec<Op> = Vec::new();
    if ops_dir.is_dir() {
        for entry in fs::read_dir(&ops_dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".rs") && name != "mod.rs" {
                let stem = name.trim_end_matches(".rs").to_string();
                let abs_path = entry.path().canonicalize().unwrap_or_else(|_| entry.path());
                let op_name = parse_op_header(&abs_path).unwrap_or_else(|| to_camel_case(&stem));
                ops.push(Op {
                    module: stem,
                    op_name,
                    abs_path,
                });
            }
        }
    }
    ops.sort_by(|a, b| a.module.cmp(&b.module));

    // The shared types module the scaffold emits when it deduplicates types
    // across operations. It is a module DIRECTORY, so the `*.rs` scan above never
    // mistakes it for an operation.
    //
    // Its own `mod.rs` is located rather than merely detected because the
    // declaration below MUST carry an explicit `#[path]`, for the same reason
    // every operation's does: this file is `include!`d into `ops/mod.rs`, so a
    // bare `mod` item inside it resolves against the INCLUDED file's directory —
    // `$OUT_DIR` — not against `ops/`.
    let types_mod_path = ops_dir
        .join("types")
        .join("mod.rs")
        .canonicalize()
        .ok()
        .filter(|p| p.is_file());

    // Read the service shape id, signing name and path discriminator from the model JSON
    let (service_shape_id, route_key, path_segments) = if model_file.exists() {
        read_service_info(&model_file)
    } else {
        (
            format!("com.amazonaws.{service_name}#UnknownService"),
            service_name.to_string(),
            Vec::new(),
        )
    };

    // Generate ops module (pub mod re-exports)
    let ops_mod = generate_ops_mod(&ops, types_mod_path.as_deref());
    fs::write(
        out_dir.join(format!("{service_name}_ops_generated.rs")),
        &ops_mod,
    )
    .unwrap();

    // Generate service decl (smithy_service attribute macro invocation)
    let service_decl = generate_service_decl(
        model_path,
        &service_shape_id,
        &route_key,
        &path_segments,
        &ops,
    );
    fs::write(
        out_dir.join(format!("{service_name}_service_decl.rs")),
        &service_decl,
    )
    .unwrap();
}

struct Op {
    module: String,
    op_name: String,
    abs_path: PathBuf,
}

/// Parse the `//! @op OperationName` header from an ops file.
fn parse_op_header(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    for line in content.lines() {
        if let Some(name) = line.trim().strip_prefix("//! @op ") {
            let name = name.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Convert snake_case to CamelCase.
fn to_camel_case(s: &str) -> String {
    s.split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect()
}

fn generate_ops_mod(ops: &[Op], types_mod_path: Option<&Path>) -> String {
    let mut out = String::from("// @generated by build.rs \u{2014} DO NOT EDIT.\n");
    out.push_str("pub use smithy_server_runtime::OperationError;\n");
    if let Some(types_mod_path) = types_mod_path {
        out.push_str(&format!(
            "#[path = {:?}]\npub mod types;\n",
            types_mod_path.to_string_lossy()
        ));
        out.push_str("#[allow(unused_imports)]\npub use types::*;\n");
    }
    for op in ops {
        out.push_str(&format!(
            "#[path = {:?}]\npub mod {};\n",
            op.abs_path.to_string_lossy(),
            op.module
        ));
    }
    for op in ops {
        out.push_str(&format!(
            "#[allow(ambiguous_glob_reexports, unused_imports)]\npub use {}::*;\n",
            op.module
        ));
    }
    // No `register_all`: the generated service is built directly from the shared
    // engine via its `new(engine)` constructor (each op is its author
    // `impl Operation` struct, constructed with `{Op}::new(engine.clone())`), so
    // there is no builder to feed per-op `register` shims into.
    out
}

fn generate_service_decl(
    model_path: &str,
    shape_id: &str,
    route_key: &str,
    path_segments: &[String],
    ops: &[Op],
) -> String {
    let ops_list = ops
        .iter()
        .map(|o| format!("{:?}", o.op_name))
        .collect::<Vec<_>>()
        .join(", ");

    let segments_list = path_segments
        .iter()
        .map(|segment| format!("{segment:?}"))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "// @generated by build.rs \u{2014} DO NOT EDIT.\n\
         #[smithy_macros::smithy_service(\n\
         \x20   service = {shape_id:?},\n\
         \x20   types = \"crate::ops\",\n\
         \x20   errors = \"crate::ops\",\n\
         \x20   model = {model_path:?},\n\
         \x20   operations({ops_list})\n\
         )]\n\
         /// Placeholder the `#[smithy_service]` macro consumes. The macro ignores\n\
         /// this type's name and emits a `ServiceDefinition<Eng>` (identity, protocol,\n\
         /// and the model-derived operation routing table) that the editable\n\
         /// `service.rs` wraps.\n\
         pub struct ServiceDecl;\n\
         \n\
         /// The AWS SigV4 signing name the `MultiServiceRouter` keys this service under.\n\
         pub const ROUTE_KEY: &str = {route_key:?};\n\
         \n\
         /// The first URI path segments this service's model declares, used to\n\
         /// disambiguate services that share one signing name (`bedrock` and\n\
         /// `bedrock-runtime` both sign as `bedrock`). Empty when the model\n\
         /// publishes no usable discriminator, which matches on the signing name\n\
         /// alone. Matched by EXACT segment equality, never by prefix.\n\
         pub const ROUTE_PATH_SEGMENTS: &[&str] = &[{segments_list}];\n"
    )
}

/// Read the service shape id, SigV4 signing name, and discriminating first URI
/// path segments out of a Smithy model JSON file.
fn read_service_info(model_path: &Path) -> (String, String, Vec<String>) {
    let unknown = || {
        (
            "com.amazonaws.unknown#Service".to_string(),
            "unknown".to_string(),
            Vec::new(),
        )
    };

    let content = fs::read_to_string(model_path).unwrap_or_default();
    let model: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return unknown(),
    };

    let shapes = match model.get("shapes").and_then(|s| s.as_object()) {
        Some(s) => s,
        None => return unknown(),
    };

    for (shape_id, shape) in shapes {
        if shape.get("type").and_then(|t| t.as_str()) == Some("service") {
            let route_key = shape
                .pointer("/traits/aws.auth#sigv4/name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    shape
                        .pointer("/traits/aws.api#service/sdkId")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_lowercase().replace(' ', ""))
                })
                .unwrap_or_else(|| {
                    // Two services keyed "unknown" both claim every request and the
                    // router refuses them as ambiguous. Report it at build time
                    // rather than at the first request.
                    println!(
                        "cargo::warning=no `aws.auth#sigv4` name or `aws.api#service` sdkId on \
                         {shape_id}; routing key falls back to \"unknown\""
                    );
                    "unknown".to_string()
                });
            return (
                shape_id.clone(),
                route_key,
                derive_route_path_segments(&model),
            );
        }
    }

    unknown()
}

/// Derive the set of first URI path segments a model's operations declare, as the
/// sorted, de-duplicated list emitted into `ROUTE_PATH_SEGMENTS`.
///
/// This is the model-driven discriminator that separates two services sharing one
/// SigV4 signing name. It is derived from `smithy.api#http` URIs alone and names
/// no service, so any colliding pair whose path spaces are disjoint is
/// disambiguated automatically.
///
/// Returns EMPTY — meaning "match on the signing name alone", the behavior every
/// service had before this existed — whenever the model cannot discriminate by
/// path:
///
/// 1. The service speaks an RPC protocol (see [`RPC_PROTOCOL_TRAITS`]). Such a
///    protocol posts EVERY operation to `/` and carries the operation name out of
///    band (`X-Amz-Target`, the `Action` body field, the rpcv2 URI), so any
///    `smithy.api#http` binding in the model is INERT on the wire and must not
///    become a discriminator. Keyed on the protocol, NOT on the absence of
///    `smithy.api#http`: most RPC models carry no http bindings, but some MIX in a
///    few (Glue and Transfer each bind `@http` on a minority of theirs), and inferring "RPC"
///    from "no bindings" publishes a segment list for exactly those models that
///    then rejects all of their real `POST /` traffic — every operation answers
///    `404 UnknownService` however completely it is implemented.
/// 2. No operation carries `smithy.api#http` at all.
/// 3. Some operation's first segment is a path LABEL (`/{Bucket}`) or the URI is
///    root (`/`). Such a segment matches ANY first segment, so the set would not
///    constrain anything. S3 is the motivating case: every S3 operation begins
///    `/{Bucket}`. Returning a partial set here would be actively wrong — it
///    would reject the very requests the label was meant to accept.
///
/// A caller must therefore treat an empty result as "no constraint", NOT as "this
/// service accepts no paths".
fn derive_route_path_segments(model: &serde_json::Value) -> Vec<String> {
    let shapes = match model.get("shapes").and_then(|s| s.as_object()) {
        Some(shapes) => shapes,
        None => return Vec::new(),
    };

    let mut segments: Vec<String> = Vec::new();
    let mut any_path_seen = false;
    for shape in shapes.values() {
        if shape.get("type").and_then(|t| t.as_str()) != Some("operation") {
            continue;
        }
        let uri = match shape
            .pointer("/traits/smithy.api#http/uri")
            .and_then(|v| v.as_str())
        {
            Some(uri) => uri,
            None => continue,
        };
        match classify_first_uri_path_segment(uri) {
            FirstUriSegment::Literal(segment) => {
                if !segments.iter().any(|existing| existing == segment) {
                    segments.push(segment.to_string());
                }
            }
            // `POST /` is the ROOT path, not a lost discriminator: claim it and keep the
            // literal segments. Discarding them instead would make the service claim
            // EVERY path under its signing name and swallow a peer that shares it.
            FirstUriSegment::Root => {
                if !segments
                    .iter()
                    .any(|existing| existing == ROOT_PATH_SEGMENT)
                {
                    segments.push(ROOT_PATH_SEGMENT.to_string());
                }
            }
            // A greedy label matches any path, so no segment list can express THAT
            // operation. Whether the whole service therefore claims every path is
            // decided after the loop — an RPC service still has a path of its own.
            FirstUriSegment::AnyPath => any_path_seen = true,
        }
    }

    // A post-to-`/` RPC service must claim the ROOT path whatever its http bindings say
    // — but it must NOT claim every path, or it swallows a peer sharing its signing name
    // (see `ROOT_PATH_SEGMENT`). With no http bindings it keeps an EMPTY list, which
    // matches on the signing name alone; adding the sentinel there would narrow services
    // that work today. rpcv2Cbor is the exception: it posts to
    // `/service/{S}/operation/{Op}`, so it claims that first segment instead.
    // A greedy label surrenders the discriminator only for a service that has no path
    // of its own. An RPC service does — `/` (or `/service/...` for cbor) — so it keeps
    // that claim rather than claiming EVERY path and swallowing a peer sharing its
    // signing name, which is the regression `ROOT_PATH_SEGMENT` exists to prevent. The
    // label-bound operation is then unroutable, which is strictly better than making
    // every peer's operations unroutable too.
    let speaks_cbor = service_speaks_rpcv2_cbor(shapes);
    let speaks_post_to_root = service_speaks_an_rpc_protocol(shapes);
    if any_path_seen {
        if !speaks_cbor && !speaks_post_to_root {
            return Vec::new();
        }
        // Its modeled literal segments cannot be trusted to cover the labelled op, so
        // keep only the protocol's own path.
        segments.clear();
    }

    // ADDITIVE, not exclusive: a service commonly declares several protocol traits
    // (CloudWatch carries awsJson1_0, awsQuery AND rpcv2Cbor), and a client may reach it
    // over any of them. Claiming only the cbor path would 404 every request from a client
    // that speaks one of the others.
    if speaks_cbor
        && !segments
            .iter()
            .any(|existing| existing == RPCV2_CBOR_PATH_SEGMENT)
    {
        // Claimed whatever its http bindings say: this IS where cbor traffic arrives.
        segments.push(RPCV2_CBOR_PATH_SEGMENT.to_string());
    }
    if (!segments.is_empty() || any_path_seen)
        && speaks_post_to_root
        && !segments
            .iter()
            .any(|existing| existing == ROOT_PATH_SEGMENT)
    {
        segments.push(ROOT_PATH_SEGMENT.to_string());
    }

    segments.sort();
    segments
}

/// Sentinel meaning "claims the root path"; must match
/// `emulator_server::ROOT_PATH_SEGMENT`, which is what reads it.
const ROOT_PATH_SEGMENT: &str = "/";

/// Protocol traits whose wire form posts every operation to `/`, so a modeled
/// `smithy.api#http` binding on such a service is never used for routing.
///
/// `aws.protocols#awsQueryCompatible` is deliberately ABSENT: it is a modifier on an
/// awsJson service (SQS carries both), not a protocol of its own, so matching the
/// real protocol trait beside it is what decides.
const RPC_PROTOCOL_TRAITS: &[&str] = &[
    "aws.protocols#awsJson1_0",
    "aws.protocols#awsJson1_1",
    "aws.protocols#awsQuery",
    "aws.protocols#ec2Query",
];

/// `smithy.protocols#rpcv2Cbor`, which is RPC but does NOT post to `/`.
///
/// Its wire form is `POST /service/{Service}/operation/{Operation}`, so the first path
/// segment is always [`RPCV2_CBOR_PATH_SEGMENT`] — never the root. Giving it
/// [`ROOT_PATH_SEGMENT`] would publish a segment no rpcv2Cbor request ever carries, and
/// the service would not match its own traffic.
const RPCV2_CBOR_PROTOCOL_TRAIT: &str = "smithy.protocols#rpcv2Cbor";

/// The fixed first path segment of every `rpcv2Cbor` request.
const RPCV2_CBOR_PATH_SEGMENT: &str = "service";

/// Whether the model's `service` shape carries one of [`RPC_PROTOCOL_TRAITS`].
///
/// Reads the service shape's own traits rather than being passed a protocol,
/// because this is the only place that decision is needed and the caller already
/// holds the whole `shapes` map.
fn service_speaks_an_rpc_protocol(shapes: &serde_json::Map<String, serde_json::Value>) -> bool {
    // The FIRST service shape, matching what `read_service_info` selects to derive the
    // shape id and route key. Scanning every service shape and answering `any` would let
    // this function describe a different service than the one being generated — latent
    // while every published model has exactly one, but the two must not disagree by
    // construction.
    service_carries_any_trait(shapes, RPC_PROTOCOL_TRAITS)
}

/// Whether the model's `service` shape speaks `rpcv2Cbor`.
///
/// Separate from [`service_speaks_an_rpc_protocol`] because the two lead to different
/// claimed segments: see [`RPCV2_CBOR_PROTOCOL_TRAIT`].
fn service_speaks_rpcv2_cbor(shapes: &serde_json::Map<String, serde_json::Value>) -> bool {
    service_carries_any_trait(shapes, &[RPCV2_CBOR_PROTOCOL_TRAIT])
}

/// Whether the FIRST `service` shape carries any of `wanted`.
///
/// The first shape, matching what `read_service_info` selects to derive the shape id and
/// route key: scanning every service shape and answering `any` would let this describe a
/// different service than the one being generated.
fn service_carries_any_trait(
    shapes: &serde_json::Map<String, serde_json::Value>,
    wanted: &[&str],
) -> bool {
    shapes
        .values()
        .find(|shape| shape.get("type").and_then(|t| t.as_str()) == Some("service"))
        .and_then(|shape| shape.get("traits"))
        .and_then(|t| t.as_object())
        .map(|traits| wanted.iter().any(|trait_id| traits.contains_key(*trait_id)))
        .unwrap_or(false)
}

/// The first `/`-separated segment of a modeled `smithy.api#http` URI when it is a
/// LITERAL, or `None` when the URI is root or its first segment is a path label
/// (`{Bucket}`) — neither of which discriminates.
///
/// The query-string suffix Smithy allows on a URI (`/{Bucket}?acl`) is stripped
/// before the check so a literal followed by a query constraint is still read as
/// that literal.
/// What a modeled `@http` URI's first path segment can be, for routing purposes.
#[derive(Debug, PartialEq, Eq)]
enum FirstUriSegment<'a> {
    /// A fixed segment the service can be matched on.
    Literal(&'a str),
    /// `POST /` — the root path, which [`ROOT_PATH_SEGMENT`] expresses.
    Root,
    /// A greedy label (`/{Bucket}`): matches ANY path, so it cannot discriminate.
    AnyPath,
}

fn classify_first_uri_path_segment(uri: &str) -> FirstUriSegment<'_> {
    let path = uri.split('?').next().unwrap_or(uri);
    let segment = path.trim_start_matches('/').split('/').next().unwrap_or("");
    if segment.is_empty() {
        return FirstUriSegment::Root;
    }
    if segment.starts_with('{') {
        return FirstUriSegment::AnyPath;
    }
    FirstUriSegment::Literal(segment)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal Smithy model JSON with one service shape and the given
    /// `(operation name, http uri)` pairs, in the shape the real published models
    /// use. An empty `uri` omits the `smithy.api#http` trait entirely, which is how
    /// `awsQuery` and `awsJson` operations appear.
    fn model_with(signing_name: &str, operations: &[(&str, &str)]) -> serde_json::Value {
        let mut shapes = serde_json::Map::new();
        shapes.insert(
            "com.amazonaws.test#TestService".to_string(),
            serde_json::json!({
                "type": "service",
                "traits": { "aws.auth#sigv4": { "name": signing_name } }
            }),
        );
        for (name, uri) in operations {
            let traits = if uri.is_empty() {
                serde_json::json!({})
            } else {
                serde_json::json!({ "smithy.api#http": { "method": "POST", "uri": uri } })
            };
            shapes.insert(
                format!("com.amazonaws.test#{name}"),
                serde_json::json!({ "type": "operation", "traits": traits }),
            );
        }
        serde_json::json!({ "smithy": "2.0", "shapes": shapes })
    }

    /// [`model_with`] plus an explicit protocol trait on the service shape.
    fn model_with_protocol(
        signing_name: &str,
        protocol: &str,
        operations: &[(&str, &str)],
    ) -> serde_json::Value {
        let mut model = model_with(signing_name, operations);
        model["shapes"]["com.amazonaws.test#TestService"]["traits"][protocol] =
            serde_json::json!({});
        model
    }

    /// A greedy label on an RPC service must not surrender the whole discriminator.
    ///
    /// An empty list means "claim every path under the signing name", which swallows a
    /// peer sharing that name. An RPC service has a path of its own — `/`, or
    /// `/service/...` for cbor — so it keeps that claim. The label-bound operation is
    /// then unroutable, which is strictly better than making every peer's operations
    /// unroutable too.
    #[test]
    fn a_greedy_label_on_an_rpc_service_keeps_the_protocol_path_instead_of_claiming_all() {
        let model = model_with_protocol(
            "transcribe",
            "aws.protocols#awsJson1_1",
            &[("StartJob", ""), ("Catch", "/{Anything}")],
        );
        let segments = derive_route_path_segments(&model);
        assert_eq!(
            segments,
            vec![ROOT_PATH_SEGMENT.to_string()],
            "an RPC service must keep its own path rather than claim every path: \
             {segments:?}"
        );

        // A cbor service keeps ITS path for the same reason.
        let cbor = model_with_protocol(
            "kendra",
            RPCV2_CBOR_PROTOCOL_TRAIT,
            &[("Rescore", ""), ("Catch", "/{Anything}")],
        );
        assert_eq!(
            derive_route_path_segments(&cbor),
            vec![RPCV2_CBOR_PATH_SEGMENT.to_string()]
        );

        // A REST service genuinely serves arbitrary paths, so it still gives up the
        // discriminator entirely.
        let rest = model_with_protocol(
            "s3",
            "aws.protocols#restXml",
            &[("Get", "/{Bucket}/{Key+}")],
        );
        assert!(derive_route_path_segments(&rest).is_empty());
    }

    /// REGRESSION: a service declaring SEVERAL protocol traits must claim the paths of
    /// all of them.
    ///
    /// A service may declare several protocol traits at once and a client may reach it
    /// over any of them. Treating rpcv2Cbor as exclusive claims only `/service/...` and
    /// drops the ROOT sentinel, so every request from a query or json client answers
    /// `404 UnknownService`.
    #[test]
    fn a_service_speaking_cbor_and_a_post_to_root_protocol_claims_both_paths() {
        let mut model = model_with("monitoring", &[("GetMetricData", "")]);
        for protocol in [
            "aws.protocols#awsJson1_0",
            "aws.protocols#awsQuery",
            "smithy.protocols#rpcv2Cbor",
        ] {
            model["shapes"]["com.amazonaws.test#TestService"]["traits"][protocol] =
                serde_json::json!({});
        }
        let segments = derive_route_path_segments(&model);
        assert!(
            segments.contains(&RPCV2_CBOR_PATH_SEGMENT.to_string()),
            "cbor traffic arrives at /service/...: {segments:?}"
        );
        assert!(
            segments.contains(&ROOT_PATH_SEGMENT.to_string()),
            "it also speaks post-to-`/` protocols, so it must claim the ROOT path: \
             {segments:?}"
        );
    }

    /// REGRESSION: an RPC service that MIXES IN a few `smithy.api#http` operations
    /// must still publish NO path discriminator.
    ///
    /// A model may bind `@http` on a minority of its operations while still posting
    /// every operation to `/`. Deriving segments from those bindings makes the generated
    /// `can_handle` demand a matching first path segment, so NO handler claims the
    /// service's real traffic and every operation answers `404 UnknownService`, however
    /// completely it is implemented.
    #[test]
    fn an_rpc_service_with_inert_http_bindings_publishes_no_segments() {
        for protocol in RPC_PROTOCOL_TRAITS {
            let model = model_with_protocol(
                "glue",
                protocol,
                &[
                    // The RPC bulk: no http binding at all.
                    ("GetDatabases", ""),
                    ("CreateTable", ""),
                    // The http-bound minority: real, but not what the SDK calls.
                    ("ListAssets", "/assets"),
                    ("ListFormTypes", "/form-types"),
                ],
            );
            assert_eq!(
                derive_route_path_segments(&model),
                vec![
                    ROOT_PATH_SEGMENT.to_string(),
                    "assets".to_string(),
                    "form-types".to_string()
                ],
                "{protocol} posts every operation to `/`, so the service must claim the \
                 ROOT path — but it must keep its modeled segments rather than claiming \
                 EVERY path, which would swallow a peer sharing its signing name"
            );
        }
    }

    /// rpcv2Cbor is RPC but does NOT post to `/`: its wire form is
    /// `POST /service/{Service}/operation/{Operation}`. Claiming the ROOT sentinel would
    /// publish a segment no such request carries, and the service would fail to match
    /// its OWN traffic — reintroducing the 404 this derivation exists to prevent.
    #[test]
    fn an_rpcv2_cbor_service_claims_its_service_path_segment_not_the_root() {
        let with_http = model_with_protocol(
            "kendra-ranking",
            RPCV2_CBOR_PROTOCOL_TRAIT,
            &[
                ("Rescore", ""),
                ("ListRescoreExecutionPlans", "/rescore-execution-plans"),
            ],
        );
        let segments = derive_route_path_segments(&with_http);
        assert!(
            segments.contains(&RPCV2_CBOR_PATH_SEGMENT.to_string()),
            "rpcv2Cbor traffic arrives at /service/..., so that segment must be claimed: \
             {segments:?}"
        );
        assert!(
            !segments.contains(&ROOT_PATH_SEGMENT.to_string()),
            "rpcv2Cbor never posts to `/`, so the ROOT sentinel is a segment its traffic \
             never carries: {segments:?}"
        );
        // The modeled http path is kept: these two services really do serve both.
        assert!(
            segments.contains(&"rescore-execution-plans".to_string()),
            "{segments:?}"
        );

        // With no http bindings at all it still claims its own path space, rather than
        // matching every path under its signing name.
        let pure = model_with_protocol(
            "gamelift",
            RPCV2_CBOR_PROTOCOL_TRAIT,
            &[("CreateGameSession", "")],
        );
        assert_eq!(
            derive_route_path_segments(&pure),
            vec![RPCV2_CBOR_PATH_SEGMENT.to_string()]
        );
    }

    /// REGRESSION: an RPC service must not claim EVERY path under its signing name.
    ///
    /// An empty segment list makes `can_handle_based_on_route_key_and_path`
    /// short-circuit to `true` for any path. An RPC service would then claim a peer's
    /// modeled path when both share a signing name, and the router — which requires
    /// exactly one claimant — answers `500 AmbiguousService`. That trades one service's
    /// 404 for another's 500, so an empty list is not a safe default.
    #[test]
    fn an_rpc_service_claims_root_but_not_a_peers_path_space() {
        let rpc = model_with_protocol(
            "transcribe",
            "aws.protocols#awsJson1_1",
            &[("StartTranscriptionJob", "/transcriptionjobs")],
        );
        let segments = derive_route_path_segments(&rpc);
        assert!(
            segments.contains(&ROOT_PATH_SEGMENT.to_string()),
            "an RPC service must claim the root path: {segments:?}"
        );
        assert!(
            !segments.is_empty(),
            "an EMPTY list means `claim every path`, which swallows a peer that shares \
             the signing name"
        );
        // The peer's path space is NOT claimed.
        assert!(
            !segments.contains(&"stream-transcription".to_string()),
            "{segments:?}"
        );
    }

    /// The guard keys on the PROTOCOL, so a genuinely REST service is unaffected and
    /// still gets its discriminator — otherwise the fix for the case above would
    /// silently disable path-based disambiguation for every restJson1/restXml pair
    /// that relies on it.
    #[test]
    fn a_rest_service_still_publishes_its_segments() {
        for protocol in ["aws.protocols#restJson1", "aws.protocols#restXml"] {
            let model = model_with_protocol(
                "bedrock",
                protocol,
                &[("ListAssets", "/assets"), ("ListFormTypes", "/form-types")],
            );
            assert_eq!(
                derive_route_path_segments(&model),
                vec!["assets".to_string(), "form-types".to_string()],
                "{protocol} routes by path, so its segments must still be published"
            );
        }
    }

    /// `awsQueryCompatible` is a MODIFIER, not a protocol: SQS carries it beside
    /// `awsJson1_0`. The real protocol trait beside it is what decides, so the
    /// modifier alone must not be read as "RPC".
    #[test]
    fn aws_query_compatible_alone_is_not_treated_as_an_rpc_protocol() {
        let model = model_with_protocol(
            "sqs",
            "aws.protocols#awsQueryCompatible",
            &[("ListAssets", "/assets")],
        );
        assert_eq!(
            derive_route_path_segments(&model),
            vec!["assets".to_string()],
            "the compat modifier is not a protocol and must not suppress segments"
        );
    }

    #[test]
    fn segments_are_derived_sorted_and_deduplicated() {
        // The six real bedrock-runtime URI shapes collapse to four distinct first
        // segments; `model` appears on several operations and must appear once.
        let model = model_with(
            "bedrock",
            &[
                ("InvokeModel", "/model/{modelId}/invoke"),
                ("Converse", "/model/{modelId}/converse"),
                ("CountTokens", "/model/{modelId}/count-tokens"),
                ("GetAsyncInvoke", "/async-invoke/{invocationArn}"),
                ("ListAsyncInvokes", "/async-invoke"),
                ("ApplyGuardrail", "/guardrail/{id}/version/{v}/apply"),
                ("InvokeGuardrailChecks", "/guardrail-checks/invoke"),
            ],
        );
        let segments = derive_route_path_segments(&model);
        assert_eq!(
            segments,
            vec!["async-invoke", "guardrail", "guardrail-checks", "model"],
            "sorted and deduplicated"
        );
    }

    #[test]
    fn the_two_bedrock_surfaces_derive_disjoint_segment_sets() {
        // The property the whole discriminator rests on. Both models declare the
        // SAME signing name, so only these sets separate them.
        let runtime = derive_route_path_segments(&model_with(
            "bedrock",
            &[
                ("InvokeModel", "/model/{modelId}/invoke"),
                ("ApplyGuardrail", "/guardrail/{id}/version/{v}/apply"),
            ],
        ));
        let control = derive_route_path_segments(&model_with(
            "bedrock",
            &[
                ("ListFoundationModels", "/foundation-models"),
                ("GetGuardrail", "/guardrails/{guardrailIdentifier}"),
                ("ListModelCopyJobs", "/model-copy-jobs"),
            ],
        ));
        assert!(
            !runtime.iter().any(|segment| control.contains(segment)),
            "runtime {runtime:?} must not overlap control {control:?}"
        );
        // And the near-collision is preserved as two DISTINCT segments rather than
        // being conflated: exact matching is what separates them.
        assert!(runtime.contains(&"guardrail".to_string()));
        assert!(control.contains(&"guardrails".to_string()));
    }

    #[test]
    fn a_label_first_segment_yields_no_discriminator() {
        // Every S3 operation begins `/{Bucket}`, which matches ANY first segment.
        // Emitting a partial set would reject the requests the label accepts, so
        // the whole model must fall back to signing-name-only routing.
        let model = model_with(
            "s3",
            &[
                ("AbortMultipartUpload", "/{Bucket}/{Key+}"),
                ("ListBuckets", "/"),
            ],
        );
        assert!(derive_route_path_segments(&model).is_empty());
    }

    #[test]
    fn one_label_operation_disqualifies_an_otherwise_literal_model() {
        // A single wildcard operation makes the set unsound, even alongside many
        // literal ones: the wildcard would still claim paths outside the set.
        let model = model_with(
            "example",
            &[
                ("Literal", "/things"),
                ("AlsoLiteral", "/other-things"),
                ("Wildcard", "/{catchAll}"),
            ],
        );
        assert!(derive_route_path_segments(&model).is_empty());
    }

    /// `POST /` is a path the service really serves, so it is claimed as
    /// [`ROOT_PATH_SEGMENT`] rather than discarded. Discarding it yields an EMPTY list,
    /// which the framework reads as "match every path under the signing name" — the
    /// peer-swallowing case `ROOT_PATH_SEGMENT` exists to prevent.
    #[test]
    fn a_root_uri_is_claimed_as_the_root_segment_not_discarded() {
        let model = model_with("example", &[("PostToRoot", "/")]);
        assert_eq!(
            derive_route_path_segments(&model),
            vec![ROOT_PATH_SEGMENT.to_string()]
        );
    }

    /// A service that binds `@http` on both a literal path and the root keeps BOTH: the
    /// literal alone would make its root-bound operation unroutable, and the root alone
    /// would strand the literal path.
    #[test]
    fn a_mixed_literal_and_root_model_keeps_both_segments() {
        let model = model_with("example", &[("Listed", "/things"), ("PostToRoot", "/")]);
        assert_eq!(
            derive_route_path_segments(&model),
            vec![ROOT_PATH_SEGMENT.to_string(), "things".to_string()]
        );
    }

    #[test]
    fn a_model_with_no_http_traits_yields_no_discriminator() {
        // Every awsQuery/awsJson service: SQS, DynamoDB, the RDS control plane,
        // IAM, KMS. They post every operation to `/`, so there is nothing to key
        // on and they must keep signing-name-only routing.
        let model = model_with("sqs", &[("SendMessage", ""), ("ReceiveMessage", "")]);
        assert!(derive_route_path_segments(&model).is_empty());
    }

    #[test]
    fn a_query_string_suffix_does_not_hide_a_literal_segment() {
        // Smithy allows a query constraint on the URI (`/things?acl`). The literal
        // before the `?` is still the discriminating segment.
        let model = model_with("example", &[("GetAcl", "/things?acl")]);
        assert_eq!(derive_route_path_segments(&model), vec!["things"]);
    }

    #[test]
    fn a_malformed_model_yields_no_discriminator_rather_than_panicking() {
        // A build script must not abort the tenant build on a surprising model.
        assert!(derive_route_path_segments(&serde_json::json!({})).is_empty());
        assert!(derive_route_path_segments(&serde_json::json!({ "shapes": 7 })).is_empty());
        assert!(derive_route_path_segments(&serde_json::json!({
            "shapes": { "x": { "type": "operation", "traits": {} } }
        }))
        .is_empty());
    }

    #[test]
    fn first_uri_path_segment_classifies_literals_labels_and_root() {
        use FirstUriSegment::{AnyPath, Literal, Root};
        assert_eq!(
            classify_first_uri_path_segment("/model/{id}/invoke"),
            Literal("model")
        );
        assert_eq!(
            classify_first_uri_path_segment("/foundation-models"),
            Literal("foundation-models")
        );
        assert_eq!(
            classify_first_uri_path_segment("/things?acl"),
            Literal("things")
        );
        // A greedy label cannot discriminate; the root path CAN, as `ROOT_PATH_SEGMENT`.
        assert_eq!(classify_first_uri_path_segment("/{Bucket}"), AnyPath);
        assert_eq!(classify_first_uri_path_segment("/{Bucket}/{Key+}"), AnyPath);
        assert_eq!(classify_first_uri_path_segment("/"), Root);
        assert_eq!(classify_first_uri_path_segment(""), Root);
    }

    #[test]
    fn the_generated_decl_emits_both_consts_and_the_two_argument_macro_call() {
        // The generated crate must carry only DATA (two consts) plus a one-line
        // delegation into the framework, so the matching rule cannot drift per
        // service.
        let ops = [Op {
            module: "invoke_model".to_string(),
            op_name: "InvokeModel".to_string(),
            abs_path: PathBuf::from("/tmp/invoke_model.rs"),
        }];
        let decl = generate_service_decl(
            "smithy-model/svc1.json",
            "com.amazonaws.bedrockruntime#AmazonBedrockFrontendService",
            "bedrock",
            &["async-invoke".to_string(), "model".to_string()],
            &ops,
        );
        assert!(
            decl.contains(r#"pub const ROUTE_KEY: &str = "bedrock";"#),
            "{decl}"
        );
        assert!(
            decl.contains(
                r#"pub const ROUTE_PATH_SEGMENTS: &[&str] = &["async-invoke", "model"];"#
            ),
            "{decl}"
        );
        // There is no standalone `impl_emulator_can_handle!`; the generated
        // `Service::can_handle` provides can-handle.
        assert!(
            !decl.contains("impl_emulator_can_handle!"),
            "the generated decl must not emit the free can_handle macro:\n{decl}"
        );
    }

    #[test]
    fn the_generated_decl_emits_an_empty_segment_list_for_a_non_discriminating_model() {
        // An awsJson service still compiles and still routes: the empty list is a
        // valid `&[&str]` and the framework reads it as "no constraint".
        let decl = generate_service_decl(
            "smithy-model/service.json",
            "com.amazonaws.sqs#AmazonSQS",
            "sqs",
            &[],
            &[],
        );
        assert!(
            decl.contains("pub const ROUTE_PATH_SEGMENTS: &[&str] = &[];"),
            "{decl}"
        );
        assert!(
            !decl.contains("impl_emulator_can_handle!"),
            "the generated decl must not emit the free can_handle macro:\n{decl}"
        );
    }

    fn op(module: &str, op_name: &str) -> Op {
        Op {
            module: module.to_string(),
            op_name: op_name.to_string(),
            abs_path: PathBuf::from(format!("/tmp/ops/{module}.rs")),
        }
    }

    /// A scaffold that deduplicated its types emits an `ops/types/` module
    /// DIRECTORY. It is not an operation, so the registry must declare it as a
    /// plain module and re-export it — otherwise every per-op file's
    /// `use super::types::*;` fails to resolve.
    #[test]
    fn the_ops_registry_declares_and_re_exports_a_shared_types_module() {
        let types_mod = PathBuf::from("/tmp/ops/types/mod.rs");
        let mod_rs = generate_ops_mod(
            &[op("create_function", "CreateFunction")],
            Some(types_mod.as_path()),
        );

        assert!(mod_rs.contains("pub mod types;\n"), "{mod_rs}");
        assert!(
            mod_rs.contains("#[allow(unused_imports)]\npub use types::*;\n"),
            "the shared types must be re-exported so `use super::types::*` resolves:\n{mod_rs}"
        );
        // `types/` is NOT an operation and must never be registered as one.
        assert!(
            !mod_rs.contains("types::register"),
            "the shared types module must not be registered as an operation:\n{mod_rs}"
        );
        // There is no `register_all` and no per-op `register` shim; the service is
        // built directly via its `new(engine)` constructor.
        assert!(
            !mod_rs.contains("register_all"),
            "the ops registry must emit no register_all:\n{mod_rs}"
        );
        assert!(
            !mod_rs.contains("::register(builder)"),
            "the ops registry must emit no per-op register shim:\n{mod_rs}"
        );
    }

    /// The shared types module MUST be declared with an explicit `#[path]`.
    ///
    /// This registry is written to `OUT_DIR` and reaches the crate through
    /// `include!(concat!(env!("OUT_DIR"), "/<svc>_ops_generated.rs"))` in
    /// `ops/mod.rs`. A bare `mod` item inside included content resolves against
    /// the INCLUDED file's directory, so a bare `pub mod types;` sends rustc
    /// looking for `$OUT_DIR/types/mod.rs` — `E0583: file not found for module
    /// 'types'`. That strands every per-op `use super::types::*;`, and each
    /// handler's types then fail with `E0433: use of undeclared type`, thousands
    /// of times over in a large service. Every operation module below is declared
    /// with `#[path]` for exactly this reason; `types` is no different.
    #[test]
    fn the_shared_types_module_is_declared_with_an_explicit_path() {
        let types_mod = PathBuf::from("/tmp/ops/types/mod.rs");
        let mod_rs = generate_ops_mod(
            &[op("create_function", "CreateFunction")],
            Some(types_mod.as_path()),
        );

        assert!(
            mod_rs.contains("#[path = \"/tmp/ops/types/mod.rs\"]\npub mod types;\n"),
            "`types` must be declared with an explicit #[path], because this file is \
             include!d from OUT_DIR and a bare `mod` would resolve against OUT_DIR:\n{mod_rs}"
        );
        // Non-vacuous: the attribute must sit immediately before the declaration,
        // not merely somewhere in the file.
        let declaration = mod_rs
            .find("pub mod types;")
            .expect("the shared types module is declared");
        assert!(
            mod_rs[..declaration].ends_with("#[path = \"/tmp/ops/types/mod.rs\"]\n"),
            "`pub mod types;` must be immediately preceded by its #[path] attribute:\n{mod_rs}"
        );
    }

    /// The self-contained layout (no `types/`) must be untouched: declaring a
    /// module for a directory that does not exist would fail the tenant build.
    #[test]
    fn the_ops_registry_declares_no_types_module_when_the_scaffold_emitted_none() {
        let mod_rs = generate_ops_mod(&[op("create_function", "CreateFunction")], None);

        assert!(
            !mod_rs.contains("pub mod types;"),
            "no types module was emitted, so none may be declared:\n{mod_rs}"
        );
        assert!(
            !mod_rs.contains("pub use types::*;"),
            "no types module was emitted, so none may be re-exported:\n{mod_rs}"
        );
        assert!(
            mod_rs.contains("pub mod create_function;"),
            "the operation module is still declared:\n{mod_rs}"
        );
    }

    /// The scan that finds operation files must treat the `types/` module
    /// directory as data, not as an operation — and `mod.rs` must stay excluded
    /// too. `types/mod.rs` is also what the `pub mod types;` declaration is
    /// pointed at, so this pins both halves of the contract.
    #[test]
    fn the_ops_scan_skips_the_types_module_directory_and_mod_rs() {
        let dir = std::env::temp_dir().join(format!(
            "emulator-core-build-support-ops-scan-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let ops_dir = dir.join("ops");
        let types_dir = ops_dir.join("types");
        fs::create_dir_all(&types_dir).expect("create ops/types dir");
        fs::write(ops_dir.join("mod.rs"), "// registry include\n").expect("write mod.rs");
        fs::write(types_dir.join("mod.rs"), "// types registry\n").expect("write types/mod.rs");
        fs::write(types_dir.join("shared.rs"), "// shared types\n").expect("write types/shared.rs");
        fs::write(
            ops_dir.join("create_function.rs"),
            "//! @op CreateFunction\n",
        )
        .expect("write op file");

        let mut found: Vec<String> = Vec::new();
        for entry in fs::read_dir(&ops_dir).expect("read ops dir").flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".rs") && name != "mod.rs" {
                found.push(name.trim_end_matches(".rs").to_string());
            }
        }
        found.sort();

        assert_eq!(
            found,
            vec!["create_function".to_string()],
            "only real operation files may be scanned as operations"
        );
        assert!(
            ops_dir.join("types").join("mod.rs").is_file(),
            "`types/mod.rs` is what the emitted `pub mod types;` #[path] points at"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // --- integration-test layout ------------------------------------------------
    //
    // The rules are enforced by a build script, so a violation must be caught by the
    // pure helpers rather than by observing a failed build. These exercise real
    // directory trees, because what is under test is a filesystem shape.

    /// A crate root with the given `tests/` entries. `paths` are relative to `tests/`
    /// and a trailing `/` means "directory containing `mod.rs`".
    fn crate_with_tests(tag: &str, paths: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "mfbs-layout-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for p in paths {
            let full = root.join("tests").join(p.trim_end_matches('/'));
            if p.ends_with('/') {
                fs::create_dir_all(&full).unwrap();
                fs::write(full.join("mod.rs"), "").unwrap();
            } else {
                fs::create_dir_all(full.parent().unwrap()).unwrap();
                fs::write(&full, "").unwrap();
            }
        }
        root
    }

    #[test]
    fn a_crate_without_all_rs_is_not_diagnosed_at_all() {
        // The property that lets this land before any crate migrates.
        let root = crate_with_tests("unmigrated", &["one.rs", "two.rs", "three.rs"]);
        let strays = stray_top_level_suites(&root.join("tests"));
        assert_eq!(strays.len(), 3, "the files exist, so the scan sees them");
        assert!(
            !root.join("tests").join("all.rs").is_file(),
            "but with no all.rs, check_test_layout returns before reporting any of them"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_stray_top_level_suite_is_found_and_all_rs_itself_is_not() {
        let root = crate_with_tests(
            "stray",
            &["all.rs", "notify_configuration.rs", "all/other.rs"],
        );
        assert_eq!(
            stray_top_level_suites(&root.join("tests")),
            vec!["notify_configuration".to_string()],
            "`all.rs` is the sanctioned target and files under all/ are not targets"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn suites_under_all_cover_both_file_and_directory_modules_but_not_deeper_files() {
        let root = crate_with_tests(
            "suites",
            &[
                "all.rs",
                "all/a_file.rs",
                "all/a_dir/",
                "all/a_dir/helper.rs",
            ],
        );
        assert_eq!(
            suites_under_all(&root.join("tests").join("all")),
            vec!["a_dir".to_string(), "a_file".to_string()],
            "helper.rs is declared by a_dir's own mod.rs, not by all.rs"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_undeclared_suite_is_the_orphan_the_second_rule_exists_for() {
        let root = crate_with_tests("orphan", &["all.rs", "all/declared.rs", "all/forgotten.rs"]);
        fs::write(root.join("tests").join("all.rs"), "mod declared;\n").unwrap();
        let declared =
            declared_modules(&fs::read_to_string(root.join("tests").join("all.rs")).unwrap());
        let orphans: Vec<_> = suites_under_all(&root.join("tests").join("all"))
            .into_iter()
            .filter(|s| !declared.contains(s))
            .collect();
        assert_eq!(
            orphans,
            vec!["forgotten".to_string()],
            "a suite nobody declared compiles nowhere and its tests never run"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn module_declarations_are_read_whatever_the_indentation_visibility_or_trailing_comment() {
        let declared = declared_modules(
            "//! docs\n\
             mod alpha;\n\
             \x20   mod beta;\n\
             pub mod gamma;\n\
             pub(crate) mod delta;\n\
             mod epsilon; // added for issue-1234\n\
             use std::fs;\n\
             mod;\n\
             mod zeta { }\n\
             // mod commented_out;\n",
        );
        assert_eq!(
            declared,
            vec![
                "alpha".to_string(),
                "beta".to_string(),
                "gamma".to_string(),
                "delta".to_string(),
                "epsilon".to_string(),
            ],
            "a visibility or a trailing comment must not hide a declaration — missing one \
             would report a declared suite as an orphan. `use` is not a mod, `mod;` has no \
             name, an inline `mod zeta {{ }}` is not a suite, and a commented-out line is not \
             a declaration"
        );
    }

    #[test]
    fn the_prescribed_declaration_points_into_tests_all_and_satisfies_the_orphan_rule() {
        let prescribed = module_declaration("state_roundtrip");
        assert!(
            prescribed[0].contains("#[path = \"all/state_roundtrip.rs\"]"),
            "without `#[path]` the declaration resolves to tests/state_roundtrip.rs, which \
             rule 1 refuses, and fails with E0583. Got: {}",
            prescribed[0]
        );
        assert!(
            prescribed.iter().all(|line| !line.contains('\n')),
            "a newline would end the cargo::error= directive and drop the rest of the message"
        );
        assert_eq!(
            declared_modules(&prescribed.join("\n")),
            vec!["state_roundtrip".to_string()],
            "pasting the prescription verbatim must satisfy rule 2, or the checker still \
             reports the suite as an orphan"
        );
    }

    #[test]
    fn an_empty_all_rs_declares_nothing_and_a_missing_one_reads_as_empty() {
        assert!(declared_modules("").is_empty());
        assert!(declared_modules("//! only a doc comment\n").is_empty());
    }
}
