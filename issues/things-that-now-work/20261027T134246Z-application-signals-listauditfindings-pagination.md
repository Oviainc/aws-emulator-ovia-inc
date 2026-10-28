Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 9c3b4064a1735206e464a3fb30963f5d1ed2eddb

# ListAuditFindings pages a per-target projection under a constant cursor tag, so a token issued for one AuditTargets set is honoured as an offset into another's findings

_Filed: 2026-10-27T13:42:46Z_
_Operation: ListAuditFindings | Class: pagination_

> _Note: this issue was identified against the code base at commit `9c3b4064a1735206e464a3fb30963f5d1ed2eddb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListAuditFindings` pages a projection that is a function of the subject the request names: `audited_findings` resolves the findings target by target from `input.audit_targets` (`@required`, `len 1..10`), plus the `Auditors` filter and the window. The cursor it pages with, however, is tagged with the bare constant `crate::state::AUDIT_FINDINGS_TOKEN_TAG`, which carries nothing about that subject, and `crate::state::page_of` cursors are numeric OFFSETS into the derived order.

So a `NextToken` handed back while paging the findings of target set A decodes successfully when replayed with target set B and is used as an offset into B's findings, silently skipping B's first N findings instead of being refused as a token this service did not issue for this projection.

The crate states the opposite rule in the very helper it declines to use here (`subject_token_tag`), applies it in every sibling per-subject paged read — `ListServiceOperations`, `ListServiceDependencies`, `ListServiceDependents` and `ListEntityEvents` via `entity_token_tag`, `ListServiceLevelObjectiveExclusionWindows` via `subject_token_tag`, and the instrumentation reads via `instrumentation_token_tag` — and the suite that pins the rule claims to cover "every per-subject paged read of this service" while omitting this one. The same facets that pick the projection out and are likewise unbound are the `Auditors` filter and the `StartTime`/`EndTime` window (the status read binds its window inside the cursor for exactly this reason).

## Evidence

The projection is the request's TARGETS' — `services/service-com-amazonaws-applicationsignals-applicationsignals/src/ops/list_audit_findings.rs:218-224`:

        let findings = state.lock().audited_findings(
            &input.audit_targets,
            input.auditors.as_deref(),
            detailed,
            &input.start_time,
            &input.end_time,
        );

…yet it is paged under a constant tag — `src/ops/list_audit_findings.rs:229-235`:

        let (audit_findings, next_token) = crate::state::page_of(
            findings,
            crate::state::AUDIT_FINDINGS_TOKEN_TAG,
            input.next_token.as_deref(),
            page_size,
        )
        .map_err(refused)?;

The state module states the rule this skips — `src/state.rs:1403-1405` (`page_of` doc):

    /// same cursor refused by a read whose cursor is a stored key. A read whose projection is ONE
    /// subject's set passes that tag with the subject bound into it ([`subject_token_tag`]),
    /// since the operation alone does not name the projection there.

and `src/state.rs:1439-1444` (`subject_token_tag` doc):

    /// A [`page_of`] cursor is an offset within one subject's ordered set, so a token issued for
    /// another subject would silently skip that subject's first entries if it were honoured.
    /// Binding the subject into the tag makes such a token fail to decode, so it is refused like
    /// any other token this service did not issue — the rule the scoped instrumentation reads
    /// already hold ([`instrumentation_token_tag`]), stated once here for every read whose set
    /// belongs to a subject the request names.

The cursor really is a bare offset — `src/state.rs:1419-1425`:

        let offset = match token {
            None => 0,
            Some(token) => decode_cursor(tag, token)
                .and_then(|cursor| cursor.parse::<usize>().ok())
                .ok_or_else(|| UNISSUED_NEXT_TOKEN.to_string())?,
        };
        let mut page: Vec<T> = items.into_iter().skip(offset).collect();

Siblings that do bind their subject into the same helper's tag:
- `src/ops/list_service_operations.rs:206-209`: `let token_tag = crate::state::entity_token_tag(crate::state::SERVICE_OPERATIONS_TOKEN_TAG, &input.key_attributes);`
- `src/ops/list_service_level_objective_exclusion_windows.rs:197-200`: `let token_tag = crate::state::subject_token_tag(crate::state::EXCLUSION_WINDOWS_TOKEN_TAG, &[Some(slo_name.as_str())]);`
- `src/ops/get_instrumentation_configuration_status.rs:231-237` even binds the WINDOW: "The window the sequence is being paged over travels inside the cursor … a request that names a DIFFERENT window than the token was issued for is paging another projection, and is refused too."

That the audit read targets a subject is stated in the projection's own doc — `src/state.rs:2901-2902`: "The audit findings the request's targets resolve to over the window, filtered by the auditors it named …" and `src/state.rs:2928-2933` loops `for target in targets { … self.audited(&target.data, …) }`.

Test coverage: the suite that pins this rule declares itself exhaustive but has no row for this read — `tests/all/paged_cursor_subject_binding.rs:12-15`:

    //! Every per-subject paged read of this service is a row of the table below, and each row
    //! carries two distinct subjects of its own: the rule generalises over WHICH read, over how
    //! the request names its subject (a path label, a key-attribute map in the body), and over
    //! which subject of that read is named — not over one recorded pair.

Its table (`tests/all/paged_cursor_subject_binding.rs:186-243`) lists only `ListServiceLevelObjectiveExclusionWindows`, `ListServiceOperations`, `ListServiceDependencies`, `ListServiceDependents` and `ListEntityEvents`. The audit suite's own page test only replays a syntactically forged token — `tests/all/audit_findings.rs:1129-1139` sends `"NextToken": "not-a-token"` — so the cross-target replay is untested.

## Steps to Reproduce

1. Create two SLOs whose SliConfig `KeyAttributes` name service `alpha` (`{"Type":"Service","Name":"alpha","Environment":"prod"}`) and two more naming service `beta`, via `POST /slo`.
2. Page alpha's findings one at a time: `POST /auditFindings?StartTime=<now-3600>&EndTime=<now+3600>` with body `{"AuditTargets":[{"Type":"service","Data":{"Service":{"Type":"Service","Name":"alpha","Environment":"prod"}}}],"MaxResults":1}` and keep the returned `NextToken` (it encodes the offset 1 under the constant tag `audit-findings/1/`).
3. Ask for beta's own first page: the same call with `"Name":"beta"` and `MaxResults=1` — note which finding comes back first.
4. Replay alpha's token against beta: the same beta call plus `"NextToken":"<token from step 2>"`.
5. Expected: HTTP 400 `ValidationException` naming `NextToken` — the token was not issued for this subject's projection, which is what `src/state.rs:1439-1444` says binding the subject into the tag guarantees and what `tests/all/paged_cursor_subject_binding.rs` pins for every sibling per-subject read.
6. Actual: HTTP 200 reporting beta's SECOND finding; beta's first finding (the one step 3 returned) is silently skipped, and a client walking beta from that token never sees it.
