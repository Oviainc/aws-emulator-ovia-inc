// SPDX-License-Identifier: CC0-1.0

//! The batch build specification: which builds a batch build is made of.
//!
//! `StartBuildBatch` starts "a batch build for a project", and which builds that batch
//! fans out into is declared by the `batch` section of the project's buildspec — the
//! section CodeBuild reads in the batch's `DOWNLOAD_BATCHSPEC` phase, and the only place
//! the identifiers of a batch's build groups are written. A `BuildGroup` reports exactly
//! those three declared members (`identifier`, `dependsOn`, `ignoreFailure`) beside the
//! summary of the build that ran for it, so this module turns the declaration into the
//! groups and nothing else: the builds themselves are minted by the handler, from the
//! project, the way a single build is.
//!
//! The buildspec is YAML, parsed with `yaml-rust2` — the pure-Rust YAML 1.2
//! implementation the other services in this workspace read caller-supplied YAML with —
//! rather than scanned as text. YAML 1.2 is a superset of JSON, so a buildspec written
//! as JSON (the `cdk.json`-style form CodeBuild also accepts) reads through the same
//! parser.

/// One build a batchspec declares, as the `BuildGroup` members it fixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchBuildDeclaration {
    /// The group's `identifier`, which is what the declaration is keyed by.
    pub identifier: String,
    /// The identifiers this group's build waits for — `depends-on` in the batchspec,
    /// `BuildGroup$dependsOn` on the wire. Empty when it waits for nothing, which is
    /// what a `build-list` entry declares.
    pub depends_on: Vec<String>,
    /// Whether a failure of this group is ignored — `ignore-failure` in the batchspec,
    /// `BuildGroup$ignoreFailure` on the wire, whose modelled default is `false`.
    pub ignore_failure: bool,
    /// The buildspec this group's build runs instead of the project's, when the entry
    /// names one (`buildspec` in the batchspec, which `StartBuild`'s
    /// `buildspecOverride` is the per-build equivalent of).
    pub buildspec: Option<String>,
}

/// How many build groups one batch build may fan out into here.
///
/// A batchspec is caller-supplied text, and the builds a batch declares are records this
/// store keeps, so the declaration is also a growth site: THIS NUMBER IS THE
/// EMULATOR'S. AWS bounds the same thing per batch with
/// `BatchRestrictions$maximumBuildsAllowed`, which a project may set lower, and this
/// ceiling is what bounds a batch whose configuration sets nothing.
pub const MAX_BATCH_BUILD_GROUPS: usize = 100;

/// What CodeBuild states as the reason a batchspec declares more builds than the batch
/// may fan out into, carried by the modelled `InvalidInputException`.
///
/// The only variable part is the ceiling itself, from the one constant that holds it; the
/// batchspec is the caller's own text and nothing of it reaches the wire.
pub fn too_many_batch_build_groups_message() -> String {
    format!(
        "Invalid input: a batch build may declare no more than {MAX_BATCH_BUILD_GROUPS} builds"
    )
}

/// How much of a buildspec is read for its batch section. THIS NUMBER IS THE EMULATOR'S:
/// `ProjectSource$buildspec` carries no `@length`, so the text is the caller's alone and
/// the parse below is bounded by this rather than by the model. It is far above any real
/// buildspec.
const MAX_BATCHSPEC_BYTES: usize = 64 * 1024;

/// How deeply a buildspec read here may nest. THIS NUMBER IS THE EMULATOR'S, and it is
/// what keeps the loader's own recursion in range: the loader descends once per nested
/// collection, so a document nested arbitrarily deep would exhaust the stack of the
/// thread serving the request. Far above any real buildspec, whose batch section nests a
/// handful of levels.
const MAX_BATCHSPEC_NESTING: usize = 64;

/// Whether `buildspec` is inside the bounds the parse below is given, checked WITHOUT
/// parsing: its size, how deeply its flow collections nest, and how deeply any one line
/// indents (a block collection needs one indentation step, or one `-`/`?` indicator, per
/// level, so the widest leading run bounds the block nesting too).
fn within_parse_bounds(buildspec: &str) -> bool {
    if buildspec.len() > MAX_BATCHSPEC_BYTES {
        return false;
    }
    let mut depth = 0usize;
    for byte in buildspec.bytes() {
        match byte {
            b'[' | b'{' => {
                depth += 1;
                if depth > MAX_BATCHSPEC_NESTING {
                    return false;
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    buildspec.lines().all(|line| {
        line.bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t' | b'-' | b'?'))
            .count()
            <= MAX_BATCHSPEC_NESTING
    })
}

/// Whether `buildspec` uses a YAML ALIAS, read from the parser's event stream.
///
/// This cannot be asked of the loaded tree: `yaml-rust2`'s loader RESOLVES an alias by
/// copying the anchored node, so by the time a tree exists the expansion has already
/// happened, and a short chain of nested aliases expands without bound. The event stream
/// reports an alias and copies nothing, so the document is walked once for this question
/// and only an alias-free one is ever handed to the loader. An anchor no alias refers to
/// expands nothing and is accepted.
fn uses_an_alias(buildspec: &str) -> bool {
    #[derive(Default)]
    struct AliasDetector {
        saw_alias: bool,
    }
    impl ::yaml_rust2::parser::EventReceiver for AliasDetector {
        fn on_event(&mut self, event: ::yaml_rust2::parser::Event) {
            if matches!(event, ::yaml_rust2::parser::Event::Alias(_)) {
                self.saw_alias = true;
            }
        }
    }

    let mut detector = AliasDetector::default();
    // A document that does not parse declares nothing either way, so a parse error here
    // is not an alias.
    let parsed = ::yaml_rust2::parser::Parser::new_from_str(buildspec).load(&mut detector, true);
    parsed.is_err() || detector.saw_alias
}

/// The builds the `batch` section of `buildspec` declares, in the order it declares
/// them, reading at most one PAST [`MAX_BATCH_BUILD_GROUPS`].
///
/// The entry past the ceiling is what lets the caller of this function tell a batchspec
/// that is within the ceiling from one that exceeds it, so a batch declaring too many
/// builds is REFUSED rather than quietly fanned out into a short list.
///
/// Two of the batchspec's build types declare their builds by identifier and are read
/// here:
///
/// * `build-list` — a list of entries, each with its own `identifier`, run in parallel;
/// * `build-graph` — the same entries plus `depends-on`, which orders them.
///
/// The third, `build-matrix`, declares no identifier of its own (CodeBuild derives one
/// per combination of the matrix's values), so it declares no build group this reading
/// can name and contributes none.
///
/// A buildspec that does not parse, carries no `batch` section, declares no entry with an
/// identifier, or falls outside the bounds this parse is given
/// ([`within_parse_bounds`], [`uses_an_alias`]) yields an empty list: there is then no
/// batch to fan out, which is what the handler reports rather than inventing a group.
pub fn batch_build_declarations(buildspec: &str) -> Vec<BatchBuildDeclaration> {
    if !within_parse_bounds(buildspec) || uses_an_alias(buildspec) {
        return Vec::new();
    }
    let Ok(mut documents) = ::yaml_rust2::YamlLoader::load_from_str(buildspec) else {
        return Vec::new();
    };
    if documents.is_empty() {
        return Vec::new();
    }
    let document = documents.remove(0);
    let Some(batch) = document.as_hash().and_then(|root| mapping_get(root, "batch")) else {
        return Vec::new();
    };
    let Some(batch) = batch.as_hash() else {
        return Vec::new();
    };
    ["build-list", "build-graph"]
        .into_iter()
        .filter_map(|build_type| mapping_get(batch, build_type))
        .filter_map(::yaml_rust2::Yaml::as_vec)
        .flatten()
        .filter_map(declaration_of)
        .take(MAX_BATCH_BUILD_GROUPS.saturating_add(1))
        .collect()
}

/// One batchspec entry as the build group it declares, or `None` when it names no
/// identifier — an entry CodeBuild has no way to address, and so no way to report.
fn declaration_of(entry: &::yaml_rust2::Yaml) -> Option<BatchBuildDeclaration> {
    let entry = entry.as_hash()?;
    let identifier = scalar_string(mapping_get(entry, "identifier")?)?;
    if identifier.is_empty() {
        return None;
    }
    Some(BatchBuildDeclaration {
        identifier,
        depends_on: mapping_get(entry, "depends-on")
            .and_then(::yaml_rust2::Yaml::as_vec)
            .map(|listed| listed.iter().filter_map(scalar_string).collect())
            .unwrap_or_default(),
        // `BuildGroup$ignoreFailure` carries the modelled default `false`, so an entry
        // that declares nothing declares a failure that is not ignored.
        ignore_failure: mapping_get(entry, "ignore-failure")
            .and_then(::yaml_rust2::Yaml::as_bool)
            .unwrap_or(false),
        buildspec: mapping_get(entry, "buildspec").and_then(scalar_string),
    })
}

/// How many declared builds each of `declarations` waits behind — its LEVEL in the
/// dependency graph the batchspec's `depends-on` members describe, aligned with
/// `declarations`.
///
/// A build group "depends on" the groups it names, so a group naming none is at level 0
/// and every other group sits one level below the deepest group it waits for. That is
/// what paces an emulated batch: the groups of one level are started together, and the
/// batch itself finishes after its deepest level has.
///
/// Computed by relaxation rather than by walking the graph, so a `depends-on` that names
/// an identifier nothing declares (which constrains nothing) or forms a cycle (which no
/// real batch can run) still terminates: one pass per declaration is enough to settle
/// every level of an acyclic graph, and a cyclic one simply stops there.
pub fn declaration_levels(declarations: &[BatchBuildDeclaration]) -> Vec<usize> {
    let mut levels = vec![0usize; declarations.len()];
    for _ in 0..declarations.len() {
        let settled = levels.clone();
        for (index, declaration) in declarations.iter().enumerate() {
            let level = declaration
                .depends_on
                .iter()
                .filter_map(|parent| {
                    declarations
                        .iter()
                        .position(|declared| &declared.identifier == parent)
                        .map(|parent| settled[parent] + 1)
                })
                .max()
                .unwrap_or(0);
            levels[index] = levels[index].max(level);
        }
    }
    levels
}

/// Look `key` up in a YAML mapping.
fn mapping_get<'doc>(
    map: &'doc ::yaml_rust2::yaml::Hash,
    key: &str,
) -> Option<&'doc ::yaml_rust2::Yaml> {
    map.iter()
        .find(|(member, _)| member.as_str() == Some(key))
        .map(|(_, value)| value)
}

/// A batchspec scalar as the string it reads as — an identifier written unquoted, as a
/// number or as a boolean is the same identifier.
fn scalar_string(value: &::yaml_rust2::Yaml) -> Option<String> {
    match value {
        ::yaml_rust2::Yaml::String(text) | ::yaml_rust2::Yaml::Real(text) => Some(text.clone()),
        ::yaml_rust2::Yaml::Integer(number) => Some(number.to_string()),
        ::yaml_rust2::Yaml::Boolean(flag) => Some(flag.to_string()),
        _ => None,
    }
}
