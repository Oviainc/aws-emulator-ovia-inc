Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a13c4bd7833d49550a5f70ddd7b2f501243c6c8c

# A check impaired by an account setting (or by another environment's license key) reports an impairedSince from before the impairment, because the onset only looks at the environment's own records

_Filed: 2026-10-28T14:44:14Z_
_Operation: GetEnvironment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a13c4bd7833d49550a5f70ddd7b2f501243c6c8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Check$impairedSince` is dated by `StoredEnvironment::health_changed_at_ms`, which takes the latest of this environment's own `modifiedAt` and the `modifiedAt` of every host and connector it holds. But two of the four checks this crate runs read facts that live OUTSIDE that set, and the crate says so itself: `KEY_COVERAGE` reads the account's `vcfPortedCoreCount` setting and `KEY_REUSE` reads the license keys of the OTHER environments in the store (`refresh_checks` takes both as parameters precisely because they are "the two facts a check of ONE environment cannot read off that environment").

So when one of those external facts changes and impairs a check, the onset stored and reported is the environment's own last-modified instant — an instant at which the check was NOT failing and, in the `KEY_COVERAGE` case, at which the setting that impairs it did not yet exist. A caller reading `Environment$checks` is told environment health "began to be impaired" before the fact that impaired it changed.

Concretely: an environment deployed with four hosts and a `licenseInfo` reports `KEY_COVERAGE` UNKNOWN while the account has stated no ported core count. Minutes later a single `PutAccountSettings` writing `vcfPortedCoreCount = 1` turns `KEY_COVERAGE` FAILED, and the `impairedSince` reported is the (minutes-old) stamp of the environment/last host, not the instant the setting was written. The same holds for `KEY_REUSE`: creating a second environment that re-uses an existing environment's VCF solution key impairs the FIRST environment's check, and that first environment's onset is dated from its own stamp, before the second environment existed.

The onset is also memoised (`impaired_since_ms.entry(..).or_insert(..)`), so the wrong instant is frozen in the store and every later read repeats it until the check passes again.

## Evidence

The onset the recomputation uses, and the doc that scopes it to the environment's own records only — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:1715-1734:
```
    /// The latest instant a fact the environment health checks read was changed: this
    /// environment's own `modifiedAt`, and that of every host and every connector it
    /// holds.
    ///
    /// A check that has started failing has been failing since no earlier than this, so
    /// it is the instant [`Self::refresh_checks`] dates an impairment from. A STORED
    /// instant, so `impairedSince` does not depend on when anybody read the environment.
    fn health_changed_at_ms(&self) -> i64 {
        self.hosts
            .values()
            .map(|host| host.modified_at_ms)
            .chain(
                self.connectors
                    .values()
                    .map(|connector| connector.modified_at_ms),
            )
            .chain(::std::iter::once(self.modified_at_ms))
            .max()
            .unwrap_or(self.modified_at_ms)
    }
```

The crate's own statement that the two other check inputs are NOT part of that set — state.rs:1739-1743:
```
    /// `key_users` counts, per license key, how many live environments of the store
    /// carry it, and `ported_core_count` is the VCF license cores the account has ported
    /// to Amazon EVS in this environment's Region: the two facts a check of ONE
    /// environment cannot read off that environment, gathered once per recomputation by
```

and the handler that changes one of them, stating it is a check input — src/ops/put_account_settings.rs:198-202:
```
        let modified = state.put_account_settings(&region, &input.settings);
        // `vcfPortedCoreCount` is what the environments of this Region check their
        // deployed cores against, so writing a setting changes a check input of every one
        // of them: their checks are recomputed from the settings this call leaves behind.
        state.recompute_environment_checks();
```

Where the wrong instant is written and frozen — state.rs:1808 and 1821-1831:
```
        let impaired_at_ms = self.health_changed_at_ms();
...
            let impaired_since_ms = if result == CHECK_FAILED {
                Some(
                    *self
                        .impaired_since_ms
                        .entry(check_type.to_string())
                        .or_insert(impaired_at_ms),
                )
```

The member being reported, from this service's model (`smithy_query(query="shape", shape="Check")`): `"impairedSince": {"target": "Timestamp", "doc": "<p>The time when environment health began to be impaired.</p>"}`.

Test coverage: the settings-driven `KEY_COVERAGE` transition is exercised but only its `result` is asserted — tests/all/environment_check_input_recompute.rs:547-593 (`writing_the_accounts_ported_core_count_recomputes_what_it_is_a_check_input_of`, rows `(required - 1, FAILED)`, …) checks `result_of(&left_behind, KEY_COVERAGE)` and never `impairedSince`. The only onset assertions are tests/all/environment_health_checks.rs:253-260 ("exactly a FAILED check carries one") and :595-624, which pin `impaired_since >= created_at` and that the onset does not move while the check keeps failing — neither pins the instant a settings- or key-driven impairment is dated from.

## Steps to Reproduce

1. CreateEnvironment with an installed VCF version, `licenseInfo`, and four `hosts` of instanceType i4i.metal; wait past the create transition.
2. GetEnvironment: the environment is CREATED, `HOST_COUNT` is PASSED and `KEY_COVERAGE` is UNKNOWN (the account has stated no vcfPortedCoreCount). Note the environment's `modifiedAt` M.
3. Do nothing for several seconds, then call PutAccountSettings with `settings: [{ "name": "vcfPortedCoreCount", "value": "1" }]` at instant P.
4. GetEnvironment again: the `KEY_COVERAGE` check is now FAILED and carries `impairedSince` = M — seconds BEFORE P, and an instant at which the check was UNKNOWN and the setting that impairs it did not exist. The expected onset is no earlier than P, the instant the fact the check reads changed.
5. KEY_REUSE variant: create environment A with solution key K and let it settle; seconds later create environment B with the same key K. GetEnvironment(A) now reports KEY_REUSE FAILED with `impairedSince` = A's own modifiedAt, dated before B (the environment that caused the reuse) existed.
