Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: b5beeec43dab1903d141b50657328b7d4cca24f8

# CreateAppMonitor's tag-key refusal bypasses the crate's single constraint-refusal wording: it quotes the ANCHORED pattern and adds a `.member` path step the walk does not write for a terminal map value

_Filed: 2026-10-29T08:53:13Z_
_Operation: CreateAppMonitor | Class: error-response_

> _Note: this issue was identified against the code base at commit `b5beeec43dab1903d141b50657328b7d4cca24f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::validation` declares itself "the ONE place a constraint refusal of this service is worded", and states two of its rules as (3) a reported `@pattern` is quoted WITHOUT the `^`/`$` pair the model writes around it, and (per `string_violation`) the path is "the dotted member path the framework's walk spells that position as". `CreateAppMonitor` routes its `Name`/`Domain`/`DomainList` refusals through that one wording (`crate::state::validation::app_monitor_refusal` -> `message`), and `crate::service::stated` routes a BOUNDARY refusal on the very same `Tags` member through it too (`Tags` is deliberately kept out of `TAGGING_MEMBER_PATHS` precisely because `CreateAppMonitor` declares it).

But the handler's tag-KEY check does not go through that wording at all. `crate::state::tag_keys_refusal` -> `tag_key_violation` builds the clause with the FRAMEWORK's `aws_smithy_schema_server::envelope` over the raw violation, so two things diverge from every other clause the same operation produces:

1. The `@pattern` is quoted WITH its anchors (`^(?!aws:)[a-zA-Z+-=._:/]+$`), because `string_violation` stores the model's pattern verbatim and only `validation::stated_kind` strips the anchor pair. A `CreateAppMonitor` refused on `Name` advertises `(?!\.)[\.\-_#A-Za-z0-9]+` (unanchored, pinned by the crate's own test), while the same operation refused on a tag key advertises the anchored expression — defeating the stated reason for rule 3 ("a caller that lifts the advertised expression out of the message and re-applies it locally must get the character grammar the service publishes").
2. The path carries an extra `member` step: `tags.<key>.member`. The vendored walk spells a TERMINAL map value with the entry KEY ALONE (`CollectionChild::Terminal => Some(key)`); `path::entry` (`<key>.member`) is used only when DESCENDING into a structure value, and a `TagMap` value is a scalar. The sibling helper in this same module, `dimension_keys_violations`, spells the analogous `DimensionKeysMap` position WITHOUT the step and cites that very vendor function as its authority — and a test pins that spelling. So this crate reports two map-entry violations of the same kind at two different path shapes, and the doc claim on `tag_key_violation` ("`path` is the dotted member path the framework would have reported for a `TagMap` entry") is not true of the value it passes.

The stated justification for the divergence is also contradicted by the handler: `tag_key_violation` says "A tag key is only ever refused by the tagging frontend, whose wording is not this service's" — yet `CreateAppMonitor` refuses a tag key itself, and its own comment says it does so "under the modelled `ValidationException` an app-monitor call answers with".

## Evidence

The crate's single wording rule — `services/service-com-amazonaws-rum-rum/src/state/validation.rs:46-49`:
```
//! them. [`message`] is this service's choice, and it is the ONE place a constraint
//! refusal of this service is worded, whether the violations were collected in front of the
//! operation or handed back by the centralized walk (`crate::service` reaches it from both)
//! or found by a check here.
```
Rule 3 of that wording — `src/state/validation.rs:61-65`:
```
//! 3. a reported `@pattern` is the expression WITHOUT the `^` / `$` pair a model writes
//!    around it to demand a whole-string match
//!    (`service_common::validation::unanchored`): a caller that lifts the advertised
//!    expression out of the message and re-applies it locally must get the character
//!    grammar the service publishes. The CHECK always runs the model's pattern as written;
```
which is applied only inside `message` — `src/state/validation.rs:673-676`:
```
fn stated_kind(kind: &ViolationKind) -> ViolationKind {
    match kind {
        ViolationKind::Pattern(regex) => ViolationKind::Pattern(unanchored(regex)),
```
The violation carries the ANCHORED pattern — `src/state/validation.rs:186-189` and `:421-423`:
```
            Self::TagKey => StringConstraints {
                min_length: 1,
                max_length: 128,
                pattern: Some(r"^(?!aws:)[a-zA-Z+-=._:/]+$"),
```
```
        match constraints.pattern {
            Some(pattern) if !matches_pattern(shape, value) => {
                ViolationKind::Pattern(pattern.to_string())
```
The tag-key path that skips `message` — `src/state.rs:2413-2419`:
```
fn tag_key_violation(path: &str, key: &str) -> Option<String> {
    let violation = validation::string_violation(validation::ConstrainedString::TagKey, path, key)?;
    ...
    let clause = ::aws_smithy_schema_server::envelope(::std::slice::from_ref(&violation));
    Some(clause.unwrap_or_else(|| violation.message()))
```
and the justification it gives, `src/state.rs:2407-2412`:
```
/// and `@pattern` itself; the WORDS stay the framework's. A tag key is only ever refused by
/// the tagging frontend, whose wording is not this service's (see that module's docs and
/// `crate::service`), so the clause here is the one the boundary would have produced —
/// quoting the model's pattern as written, anchors and all. `path` is the dotted member
/// path the framework would have reported for a `TagMap` entry, and the key is echoed into
/// the clause exactly as the caller sent it.
```
The handler that contradicts "only ever refused by the tagging frontend" — `src/ops/create_app_monitor.rs:281-288`:
```
        // Both are refused before anything is written, under the modelled
        // `ValidationException` an app-monitor call answers with.
        if let Some(message) = crate::state::tag_keys_refusal(tags.keys().map(String::as_str)) {
            return Err(
                CreateAppMonitorErrorModeled::ValidationException(ValidationException { message })
                    .into(),
            );
        }
```
while the SAME handler words its other constraint refusals through `message` — `src/ops/create_app_monitor.rs:210-219` (`app_monitor_refusal`, which ends in `message(&violations, &lists)` at `src/state/validation.rs:483`).

The extra path step — `src/state.rs:2436-2437`:
```
    keys.into_iter()
        .find_map(|key| tag_key_violation(&format!("tags.{key}.member"), key))
```
against the sibling helper in the same module, `src/state.rs:1814-1818`:
```
            // An entry is reported at the key the CALLER chose: that is the segment the walk
            // spells a terminal map value with (`begin_collection_child` in
            // `vendor/smithy-schema-server/src/constraints.rs`), and it is the position both
            // halves of the entry hold.
            let entry = format!("{path}.{DIMENSION_KEYS_MEMBER}.{field}");
```
and against the vendored rule both cite, `vendor/smithy-schema-server/src/constraints.rs:1142-1147`:
```
                    CollectionChild::Descending => Some(path::entry(&key)),
                    // The entry's value IS what failed. A service reports this at
                    // the map member with a bundled requirement and no key at all;
                    // we report the key instead. See
                    // `map_value_pattern_reports_key_path`.
                    CollectionChild::Terminal => Some(key),
```
(`path::entry` is the `<key>.member` form — `vendor/smithy-schema-server/src/constraints.rs:88-93`: "A map entry names its key, then the collection step." / `format!("{key}.{COLLECTION_STEP}")`.)

Test coverage. The unanchored form IS pinned for this operation's other members — `tests/all/constraint_refusals.rs:44-51`:
```
/// `AppMonitorDomain`'s `@pattern` as a refusal quotes it: the modelled expression without
/// the `^` / `$` pair the model writes around it to demand a whole-string match.
...
const NAME_PATTERN: &str = r"(?!\.)[\.\-_#A-Za-z0-9]+";
```
used at `tests/all/constraint_refusals.rs:327-331` and `:352-357`. The no-`.member` map-entry path is pinned for `DimensionKeys` — `tests/all/dimension_keys_constraints.rs:130`:
```
                "Value '{value}' at '{prefix}.{DIMENSION_KEYS_MEMBER}.{}' failed to satisfy \
```
NOTHING pins the tag-key clause's pattern spelling or its path: the only assertion on that message is `tests/all/create_app_monitor_tags.rs:313-316`, which checks only that the key appears —
```
            let message = crate::support::message(&body).unwrap_or_default();
            assert!(
                message.contains(row.key.as_str()),
                "case {case:?}: the refusal must name the key it refused, got {message:?}"
```
so no existing test states a reason for the divergence and none would change if the clause went through `crate::state::validation::message` at the `tags.<key>` path.

## Steps to Reproduce

1. POST /appmonitor with {"Name": ".bad", "Domain": "example.com"} — 400 ValidationException whose message is `1 validation error detected: Value '.bad' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: (?!\\.)[\\.\\-_#A-Za-z0-9]+` (pattern UNANCHORED, path has no collection step).
2. POST /appmonitor with {"Name": "m", "Domain": "example.com", "Tags": {"aws:owner": "x"}} — 400 ValidationException whose message instead reports the path `tags.aws:owner.member` and the ANCHORED pattern `^(?!aws:)[a-zA-Z+-=._:/]+$`. Expected the same wording rule the step above produces: the pattern without its `^`/`$` pair, at the path `tags.aws:owner` (the key alone, which is how the walk spells a terminal map value).
3. For contrast inside the crate, POST /rummetrics/{name}/metrics with a DimensionKeys entry whose field path breaks `DimensionKey`'s `@length`: the clause reports `metricDefinition.dimensionKeys.<field>` / `metricDefinitions.1.member.dimensionKeys.<field>` — the analogous map entry with NO `.member` step, as `tests/all/dimension_keys_constraints.rs:130` pins.
