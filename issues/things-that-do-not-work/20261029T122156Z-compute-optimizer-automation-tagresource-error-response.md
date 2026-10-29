Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 4ff8cb4126a9fd6df9537728614f77c373f30083

# The two tag-key rules are applied in opposite order on the two tag write paths, so one bad tag list is refused two different ways

_Filed: 2026-10-29T12:21:56Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `4ff8cb4126a9fd6df9537728614f77c373f30083`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A caller-supplied `tags` list is judged against exactly two rules this crate owns: no repeated key (`crate::state::duplicate_tag_key_violation`) and no key outside the allowed form / inside the reserved `aws:` namespace (`crate::state::tag_keys_violation`). Both write paths apply both rules, but in OPPOSITE order: `CreateAutomationRule` checks `tag_keys_violation` first and `duplicate_tag_key_violation` last, while `TagResource` checks `duplicate_tag_key_violation` first and `tag_keys_violation` second. A request whose tag list breaks both rules at once — e.g. `[{"key":"aws:owner"},{"key":"team"},{"key":"team"}]` — is therefore refused with the reserved-namespace message by the create path and with the duplicate-key validation report by the tagging path, for one and the same input.

Both handlers document the opposite: TagResource's own comment says its duplicate-key check is "The same check the create path makes of the `tags` it stores", and CreateAutomationRule's handler doc says its tag clauses are "what the tagging operations refuse the same key with". The crate deliberately single-homes error wording in `src/state.rs` ("Every client error this service raises is worded HERE, so the body a caller parses is built in one place and two handlers raising the same error cannot word it differently"), and this ordering difference defeats that for the one input that fails both rules.

## Evidence

`TagResource` runs the duplicate rule first — `src/ops/tag_resource.rs:180`-`196`:
```
        // A tag list states one value per key, so a request naming the same key
        // twice states two values for one tag. Refused ahead of the token ledger
        // and of the store, so it consumes nothing and changes nothing. The same
        // check the create path makes of the `tags` it stores.
        if let Some(report) = crate::state::duplicate_tag_key_violation(&input.tags) {
            return Err(invalid_parameter(report));
        }
        ...
        if let Some(message) =
            crate::state::tag_keys_violation(input.tags.iter().map(|tag| tag.key.as_str()))
        {
            return Err(invalid_parameter(message));
        }
```
`CreateAutomationRule` runs them the other way round — `src/ops/create_automation_rule.rs:471`-`487`:
```
    if let Some(report) =
        crate::state::tag_keys_violation(input.tags.iter().flatten().map(|tag| tag.key.as_str()))
    {
        return Some(report);
    }
    ...
    input
        .tags
        .as_deref()
        .and_then(crate::state::duplicate_tag_key_violation)
}
```
while its own doc claims parity with the tagging path — `src/ops/create_automation_rule.rs:434`-`437`:
```
/// * `tags` — one request may not name the same tag key twice
///   ([`crate::state::duplicate_tag_key_violation`]), and no key may be one a
///   caller may not supply ([`crate::state::tag_keys_violation`]), which is
///   what the tagging operations refuse the same key with.
```
The two messages are different bodies, both minted in `src/state.rs`: `reserved_tag_key_message()` (state.rs:409) "Tag keys beginning with aws: are reserved for Amazon Web Services use and cannot be specified" versus `duplicate_tag_key_violation`'s `constraint_report("tags", "Member must not name the same tag key more than once")` (state.rs:2697-2700).

Test coverage: `tests/all/reserved_tag_keys.rs` drives a reserved key through the create path (lines 137-172) and through `TagResource` (lines 178-224), but every case carries at most one faulty key, so no existing test pins which of the two rules wins for a list that breaks both — the divergence is unpinned.

## Steps to Reproduce

1. Start the emulator and call UpdateEnrollmentConfiguration with `{"status":"Active"}`.
2. Call CreateAutomationRule with a valid rule and `"tags":[{"key":"aws:owner","value":"x"},{"key":"team","value":"a"},{"key":"team","value":"b"}]` — the list both repeats `team` and names a reserved key. Observe the message: the reserved-namespace sentence.
3. Create the same rule without tags, note its `ruleArn`/`ruleRevision`, then call TagResource with the identical `tags` list.
4. Observe that TagResource answers "1 validation error detected: Value at 'tags' failed to satisfy constraint: Member must not name the same tag key more than once" instead — the same input, refused for a different reason, although both handlers document the checks as identical.
