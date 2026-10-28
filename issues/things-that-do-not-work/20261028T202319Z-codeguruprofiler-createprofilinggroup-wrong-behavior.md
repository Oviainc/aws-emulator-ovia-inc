Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a08233279ea00f458c8f357984c10e06f076154f

# A second create reusing a clientToken silently overwrites the token's idempotency record, so retrying the FIRST create conflicts instead of replaying

_Filed: 2026-10-28T20:23:19Z_
_Operation: CreateProfilingGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a08233279ea00f458c8f357984c10e06f076154f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`create_tokens` is a `clientToken -> group name` map whose documented purpose is "so a retry of the same create is idempotent instead of a conflict" (src/state.rs:1307-1308). `CreateProfilingGroup` writes that entry with an UNCONDITIONAL `HashMap::insert` at the end of every successful create (src/ops/create_profiling_group.rs:280-282), so a create that reuses a `clientToken` already associated with another group name REPLACES the stored association instead of keeping it.

The handler's own comment states that such a request "is a different create and falls through to the name-conflict check below" (src/ops/create_profiling_group.rs:193-194) — it says nothing about the token record being rewritten, and the fall-through only describes the case where the second name already exists. When the second name does NOT exist, the second create succeeds and the token now names the second group, and the first create's idempotency record is gone.

The observable consequence: a retry of the FIRST create — the exact failure-and-retry the `@idempotencyToken` member exists for — no longer takes the replay branch at src/ops/create_profiling_group.rs:195-205 (the stored name no longer equals the requested one), falls into `guard.groups.contains_key(..)` at line 207 and answers 409 `ConflictException`. The store has lost state a later caller can observe: one token's create record was dropped, and the operation that was supposed to be idempotent on it now reports a conflict.

The crate treats this map elsewhere as the record of "the create tokens that made this group" (src/ops/delete_profiling_group.rs:160-161), which is the same assumption the overwrite breaks: after the overwrite the first group has no token recorded against it at all, so deleting the first group leaves the token entry behind (it names the second group) while a retry of the first create can never be replayed again.

## Evidence

Documented purpose of the store field — src/state.rs:1306-1312:
```
    /// `CreateProfilingGroup`'s `clientToken` -> the name it created, so a retry of
    /// the same create is idempotent instead of a conflict. Bounded by `groups`: an
    /// entry is written only by a create that also added a group, and
    /// `DeleteProfilingGroup` removes every entry naming the group it deletes, so this
    /// map never holds more entries than there are live groups.
    pub create_tokens: HashMap<String, String>,
```

The unconditional overwrite that breaks it — src/ops/create_profiling_group.rs:277-282:
```
        guard
            .groups
            .insert(input.profiling_group_name.clone(), group);
        guard
            .create_tokens
            .insert(input.client_token.clone(), input.profiling_group_name.clone());
```

The replay branch that then stops matching — src/ops/create_profiling_group.rs:192-205:
```
        // Idempotency: the same `clientToken` that already created THIS name replays the
        // stored group. A used token naming a different group is a different create and
        // falls through to the name-conflict check below.
        if guard
            .create_tokens
            .get(&input.client_token)
            .is_some_and(|name| name == &input.profiling_group_name)
        {
```
and the conflict the retry gets instead — src/ops/create_profiling_group.rs:207-219:
```
        if guard.groups.contains_key(&input.profiling_group_name) {
            return Err(CreateProfilingGroupErrorModeled::ConflictException(
```

The same map read as "the create tokens that made this group" — src/ops/delete_profiling_group.rs:160-164:
```
        // Forget the create tokens that made this group: a token kept after the delete
        // would make a later CreateProfilingGroup replay a group that no longer exists.
        guard
            .create_tokens
            .retain(|_, name| name != &input.profiling_group_name);
```

Model text for the member whose contract this is (`smithy_query(query="doc", shape="CreateProfilingGroupRequest", member="clientToken")`, restated in the handler's own contract block at src/ops/create_profiling_group.rs:52-54): "Amazon CodeGuru Profiler uses this universally unique identifier (UUID) to prevent the accidental creation of duplicate profiling groups if there are failures and retries."

Test coverage: tests/all/profiling_group_lifecycle.rs:647-672 (`a_repeated_create_is_idempotent_on_its_client_token_and_conflicts_without_it`) pins only (a) same token + same name replays and (b) a NEW token on an existing name is 409; tests/all/profiling_group_lifecycle.rs:1127-1129 pins the same token after a delete creating a fresh group. No test reuses one token for two different names, so nothing pins the behaviour reported here.

## Steps to Reproduce

1. POST /profilingGroups?clientToken=tok-1 with {"profilingGroupName":"alpha"} — 201, and src/ops/create_profiling_group.rs:280-282 stores create_tokens{"tok-1" -> "alpha"}.
2. POST /profilingGroups?clientToken=tok-1 with {"profilingGroupName":"beta"} — the replay branch (lines 195-205) does not match ("alpha" != "beta"), "beta" does not exist so there is no conflict, the group is created and line 280-282 overwrites the entry to create_tokens{"tok-1" -> "beta"}; the record of the create of "alpha" is gone.
3. Retry the step-1 request: POST /profilingGroups?clientToken=tok-1 with {"profilingGroupName":"alpha"} — the replay branch still does not match, `guard.groups.contains_key("alpha")` is true, and the response is 409 ConflictException instead of the stored "alpha" group the token created.
4. Contrast with src/state.rs:1307-1308, which states the map exists "so a retry of the same create is idempotent instead of a conflict".
