// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a request the framework refused before any handler ran.
//!
//! The framework hands the refusal over structurally — a list of
//! [`aws_smithy_schema_server::Violation`]s, each naming the member, what it failed and
//! what it held — and leaves the sentence to the service being emulated
//! (`smithy_server_runtime::Refusal`). CodeBuild does not state every one of them the
//! same way: a few constraint classes have a sentence of CodeBuild's OWN, worded per
//! RULE and built from the request, and everything else keeps the shared constraint
//! envelope that names the member and the constraint it failed.
//!
//! One module so that the choice is made in one place for every operation of the
//! service: a refusal carries no operation name, so each sentence below is keyed to the
//! rule that was broken — the member path the model gives it and the constraint trait the
//! model puts on it — never to one operation or one recorded request.

use aws_smithy_schema::{Schema, ShapeType};
use aws_smithy_schema_server::constraints::RangeTrait;
use aws_smithy_schema_server::{Violation, ViolationKind};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

/// The whole message for a request refused by the constraint walk, as CodeBuild words
/// it.
///
/// A violation of a rule CodeBuild states itself is the WHOLE answer whenever one is
/// among the violations found — the service refuses the rule that was broken rather than
/// reporting a constraint envelope beside it — and the first such violation in the
/// walk's own order decides the sentence. Everything else is the shared envelope, with
/// each clause rendered as this service renders it ([`stated_clause`]).
pub(super) fn message(violations: &[Violation]) -> String {
    if let Some(stated) = violations.iter().find_map(codebuild_sentence) {
        return stated;
    }
    let stated: Vec<Violation> = violations.iter().map(stated_clause).collect();
    aws_smithy_schema_server::envelope(&stated).unwrap_or_else(|| "Invalid input".to_string())
}

/// The sentence CodeBuild states for `violation`, when that violation is of a rule it
/// words itself rather than through the constraint envelope.
fn codebuild_sentence(violation: &Violation) -> Option<String> {
    let path = member_path_of(&violation.path);
    match &violation.kind {
        // A sort order outside its enum: CodeBuild's own sentence, so no caller is told
        // its list request failed generic request validation.
        ViolationKind::EnumValue(_) if is_sort_order(violation) => {
            Some(INVALID_SORT_ORDER_MESSAGE.to_string())
        }
        // An empty input list of build ids.
        ViolationKind::MinLength(_)
            if collection_size(violation).is_some() && path == BUILD_ID_LIST_PATH =>
        {
            Some(EMPTY_BUILD_ID_LIST_MESSAGE.to_string())
        }
        // An input list of resource identifiers longer than the model lets a request
        // carry.
        ViolationKind::MaxLength(max) if IDENTIFIER_LIST_PATHS.contains(&path.as_str()) => {
            collection_size(violation).map(|size| {
                format!(
                    "The maximum length of input resource list is {max} but input list \
                     with size {size} is found."
                )
            })
        }
        // A build timeout outside the window the model pins on it.
        ViolationKind::MinValue(_) | ViolationKind::MaxValue(_) => {
            build_timeout_windows().get(&path).map(stated_window)
        }
        _ => None,
    }
}

/// What CodeBuild states as the reason a list request asked for a sort order it does
/// not define, carried by the modelled `InvalidInputException`.
///
/// The sentence is FIXED: it quotes neither the rejected value, the member path nor the
/// permitted set, so it is the same on every request and every operation that takes a
/// sort order. Recorded behaviour — the model states no message for the error.
const INVALID_SORT_ORDER_MESSAGE: &str = "Invalid sort order specified";

/// What CodeBuild states as the reason a batch request names no build to act on,
/// carried by the modelled `InvalidInputException`.
///
/// Fixed text that names the KIND of list the request left empty rather than the member
/// path or the size it carried, so the sentence is the same for every such request.
/// Recorded behaviour — the model states no message for the error.
const EMPTY_BUILD_ID_LIST_MESSAGE: &str = "The input build ID list cannot be empty.";

/// The validation path of the input member that carries a list of BUILD ids.
///
/// `BatchGetBuildsInput$ids` and `BatchDeleteBuildsInput$ids` both target `BuildIds`,
/// the one list of this service's inputs that is `@length(min: 1)` and holds build ids,
/// so an empty-list violation at this path is a request that named no build. The
/// sibling `ids` members that hold other identifiers cannot reach the sentence above:
/// `BatchGetBuildBatchesInput$ids` is `@length(min: 0)` and `BatchGetSandboxesInput$ids`
/// carries no `@length` at all, so neither has a minimum to violate.
const BUILD_ID_LIST_PATH: &str = "ids";

/// The validation paths of the input members that carry a list of RESOURCE IDENTIFIERS —
/// the lists CodeBuild's over-long-list sentence calls an "input resource list".
///
/// Every batch operation of this service takes exactly one such member, and these are
/// their names: `ids` (`BuildIds` on the build read and delete, `BuildBatchIds`,
/// `SandboxIds`), `names` (`ProjectNames`, `FleetNames`), `reportGroupArns`,
/// `reportArns` and `commandExecutionIds`. Keyed to them rather than applied to every
/// collection the model bounds, because this service bounds lists that are NOT identifier
/// lists — a project's `tags` (`TagList`, max 50), its `secondarySources`
/// (`ProjectSources`, max 12) and their siblings — and a request refused for one of those
/// is not a request whose resource list was too long, so it keeps the shared constraint
/// envelope naming the member and the constraint it failed.
const IDENTIFIER_LIST_PATHS: [&str; 5] = [
    "ids",
    "names",
    "reportGroupArns",
    "reportArns",
    "commandExecutionIds",
];

/// `true` when `violation` is a value outside the model's `SortOrderType` value set.
///
/// A violation carries the value set from the schema of the member that failed, so the
/// comparison is against the MODEL rather than against a member name: every operation
/// whose input models a sort order is covered, including the ones whose handlers are
/// still scaffolded, and no other enum of this service declares the same values.
fn is_sort_order(violation: &Violation) -> bool {
    let declared = crate::ops::types::SORT_ORDER_TYPE_VALUES;
    match &violation.kind {
        // Set equality, so the order codegen happens to list an enum's values in is
        // not part of the test.
        ViolationKind::EnumValue(allowed) => {
            allowed.len() == declared.len()
                && allowed
                    .iter()
                    .all(|value| declared.contains(&value.as_str()))
        }
        _ => false,
    }
}

/// The element count a COLLECTION-length violation carries, or `None` for a violation of
/// any other kind of length.
///
/// The constraint walk renders a collection's value as its element count followed by
/// [`ITEMS_SUFFIX`] (`101 items`), where a string's is the string itself — so this is
/// what tells a list that is too long apart from a string that is, and it is what the
/// size CodeBuild states in its own sentence is read from.
fn collection_size(violation: &Violation) -> Option<usize> {
    violation
        .value
        .as_deref()?
        .strip_suffix(ITEMS_SUFFIX)?
        .parse()
        .ok()
}

/// How the constraint walk spells the element count of a collection it refused.
const ITEMS_SUFFIX: &str = " items";

/// The window a build timeout member is held to, as the sentence CodeBuild states for a
/// value outside it.
///
/// Both bounds come from the `@range` the MODEL puts on the member, and the maximum is
/// stated in hours exactly when it is a whole number of them — which is how AWS words
/// the window of `BuildTimeOut` (`@range(min: 5, max: 2160)`): "between 5 minutes and 36
/// hours". Recorded behaviour — the model states no message for the error.
fn stated_window(window: &RangeTrait) -> String {
    format!(
        "Invalid timeout. Should be between {} and {}",
        window.min.map_or_else(|| "0 minutes".to_string(), minutes),
        window.max.map_or_else(|| "0 minutes".to_string(), minutes),
    )
}

/// `minutes` worth of wait, in the unit AWS states it in: whole hours where it is a
/// whole number of them, minutes otherwise.
fn minutes(minutes: f64) -> String {
    let whole = minutes.max(0.0).round() as i64;
    /// Minutes in an hour, for the unit the window above is stated in.
    const MINUTES_PER_HOUR: i64 = 60;
    if whole >= MINUTES_PER_HOUR && whole % MINUTES_PER_HOUR == 0 {
        return format!("{} hours", whole / MINUTES_PER_HOUR);
    }
    format!("{whole} minutes")
}

/// The `@range` window of every input member that states a BUILD's timeout, by
/// validation path.
///
/// Built once from the schemas themselves, so nothing here names a bound: a path is in
/// this map exactly when the MODEL puts a `@range` on a member whose name is one of
/// [`BUILD_TIMEOUT_MEMBERS`], and the window stated is that member's own. The queued
/// timeout is a different wait with a window of its own (`TimeOut`, 5..480) and is not
/// among them.
fn build_timeout_windows() -> &'static BTreeMap<String, RangeTrait> {
    static WINDOWS: LazyLock<BTreeMap<String, RangeTrait>> = LazyLock::new(|| {
        let mut windows = BTreeMap::new();
        for input in super::INPUT_SCHEMAS {
            for member in input.members() {
                let Some(name) = member.member_name() else {
                    continue;
                };
                if !BUILD_TIMEOUT_MEMBERS.contains(&name) {
                    continue;
                }
                if let Some(window) = range_of(member) {
                    windows.insert(
                        aws_smithy_schema_server::constraints::path::member(name),
                        window,
                    );
                }
            }
        }
        windows
    });
    &WINDOWS
}

/// The names the model gives the members that state how long a BUILD may run before
/// CodeBuild times it out — the member of a project's configuration, and the override a
/// start takes for one build only. Both target `BuildTimeOut`, whose window AWS words as
/// "from 5 to 2160 (36 hours)".
const BUILD_TIMEOUT_MEMBERS: [&str; 2] = ["timeoutInMinutes", "timeoutInMinutesOverride"];

/// The `@range` trait codegen attached to `member`, if the model puts one on it.
fn range_of(member: &Schema) -> Option<RangeTrait> {
    let carried = member.traits()?.get(&RangeTrait::TRAIT_ID)?;
    carried.as_any().downcast_ref::<RangeTrait>().copied()
}

/// `violation` with the offending value rendered in the wire form of the member's
/// own modelled type, and an enumerated value set stated in the order CodeBuild
/// states it.
///
/// AWS states a constraint violation as `Value '<value>' at '<member path>' failed
/// to satisfy constraint: …`, and the value it echoes is written the way that
/// member's type is written on the wire: a floating-point member always carries a
/// fractional part (`150` sent for a `double` is stated as `'150.0'`), an integral
/// member never does. The framework renders every numeric leaf with Rust's
/// `to_string`, which prints an integral `f64` as `150`, so this restores the point
/// for exactly the members [`floating_point_member_paths`] reports as floating
/// point. Every other violation — and any value that is not a number — keeps the value
/// the framework worded it with.
fn stated_clause(violation: &Violation) -> Violation {
    let stated = sorted_value_set(violation);
    let Some(shown) = stated.value.as_deref() else {
        return stated;
    };
    if !floating_point_member_paths().contains(&member_path_of(&violation.path)) {
        return stated;
    }
    let Ok(value) = shown.parse::<f64>() else {
        return stated;
    };
    Violation {
        // `{value:?}` is the shortest text that round-trips an `f64` AND always
        // carries a point or an exponent; `to_string` would print `150` again.
        value: Some(format!("{value:?}")),
        ..stated
    }
}

/// `violation` with an enumerated value set stated in ascending order.
///
/// CodeBuild lists the values of an enumerated type in that order rather than in the
/// order its model declares them (recorded behaviour: `ProjectVisibilityType` is
/// declared `PUBLIC_READ, PRIVATE` and stated as `[PRIVATE, PUBLIC_READ]`), and the
/// order is part of the sentence a client matches on. A violation of any other kind is
/// left exactly as the framework worded it.
fn sorted_value_set(violation: &Violation) -> Violation {
    let ViolationKind::EnumValue(allowed) = &violation.kind else {
        return violation.clone();
    };
    let mut stated: Vec<String> = allowed.clone();
    stated.sort();
    Violation {
        kind: ViolationKind::EnumValue(stated),
        ..violation.clone()
    }
}

/// The validation paths, across every input in [`super::INPUT_SCHEMAS`], of the members
/// the model types as `float` or `double`.
///
/// Built once from the schemas themselves, so nothing here names a member: a path is in
/// this set exactly when the MODEL says that member is floating point.
/// Codegen emits a member schema without its target's own members
/// (`Schema::new_member` takes a shape type and a name), so the walk sees each
/// input's top-level members — which is where this service's floating-point
/// members are (`DescribeCodeCoverages$minLineCoveragePercentage` and
/// `$maxLineCoveragePercentage`, both targeting `Percentage`).
fn floating_point_member_paths() -> &'static BTreeSet<String> {
    static PATHS: LazyLock<BTreeSet<String>> = LazyLock::new(|| {
        let mut paths = BTreeSet::new();
        for input in super::INPUT_SCHEMAS {
            for member in input.members() {
                let Some(name) = member.member_name() else {
                    continue;
                };
                if matches!(member.shape_type(), ShapeType::Float | ShapeType::Double) {
                    // The casing of a validation path segment belongs to the path,
                    // not to the model, so it is spelled by the framework module
                    // that owns that spelling.
                    paths.insert(aws_smithy_schema_server::constraints::path::member(name));
                }
            }
        }
        paths
    });
    &PATHS
}

/// `path` with the position or key of each collection step dropped, so a member
/// inside a list or a map answers to the one path its member schema has.
///
/// A violation path spells a collection step as `<position-or-key>.member`
/// (`ids.1.member`); the model knows that member only as `ids.member`.
fn member_path_of(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('.') {
        // The caller-chosen key of a map entry could itself be the literal
        // `member`; that only ever costs this one path a match, since the key is
        // dropped either way.
        if segment == COLLECTION_STEP {
            segments.pop();
        }
        segments.push(segment);
    }
    segments.join(".")
}

/// The segment a validation path puts between a collection and its element, as
/// `aws_smithy_schema_server::constraints::path` spells it.
const COLLECTION_STEP: &str = "member";
