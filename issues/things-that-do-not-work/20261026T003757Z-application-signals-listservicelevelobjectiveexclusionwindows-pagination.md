Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 232a76c0adfd6241e7fe920f3ef518cb863b87b4

# Bind the paged subject into the exclusion-window NextToken tag: a token issued for one SLO is honoured as an offset into another's set

_Filed: 2026-10-26T00:37:57Z_
_Operation: ListServiceLevelObjectiveExclusionWindows | Class: pagination_

> _Note: this issue was identified against the code base at commit `232a76c0adfd6241e7fe920f3ef518cb863b87b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListServiceLevelObjectiveExclusionWindows` pages a PER-SLO projection (the window set of the SLO its `@httpLabel` `Id` names) but tags its cursor with a constant that carries nothing about that SLO. `crate::state::page_of` therefore decodes a token issued while paging SLO A and uses the offset it carries against SLO B's window list, silently skipping B's first N windows instead of refusing a token that was not issued for this projection.

This is precisely the failure the same crate's instrumentation cursor binds its subject into the tag to prevent, and the rule `page_of`'s own doc states ("a token is only ever resolved against the projection that issued it"). The exclusion-window read skips that binding, so the two paged reads of this crate disagree on whether a foreign cursor is refused or honoured.

The binding must be on the RESOLVED SLO name rather than on the raw `Id`, so a token handed back for a read by bare name still resumes a read by ARN — the behaviour `tests/all/exclusion_window_reads.rs::either_id_form_answers_the_same_page` pins.

## Evidence

The rule `page_of` claims to enforce — `services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:1357-1360`:

    /// is an offset into their deterministic order, not a key of a stored record. Each read
    /// passes its own [`SERVICE_OPERATIONS_TOKEN_TAG`]-style tag, so a token is only ever
    /// resolved against the projection that issued it; `Err` carries the refusal message for a
    /// token that is not one this service issued under that tag.

The sibling cursor that DOES bind its subject, and states why — `src/state.rs:3781-3784`:

    /// A cursor is a key within ONE scope's ordered set, so a token issued for another scope
    /// would silently skip entries if it were honoured. Binding the scope into the tag makes
    /// such a token fail to decode, and it is refused like any other token this service did not
    /// issue.

…implemented at `src/state.rs:3785-3792` (`instrumentation_token_tag` hashes `instrumentation_type`/`service`/`environment` into the tag), and again per configuration AND status at `src/state.rs:3368-3380` (`instrumentation_status_token_tag`).

The exclusion-window tag carries no subject — `src/state.rs:1612-1613`:

    /// [`SERVICE_OPERATIONS_TOKEN_TAG`], for `ListServiceLevelObjectiveExclusionWindows`.
    pub const EXCLUSION_WINDOWS_TOKEN_TAG: &str = "exclusion-windows/1/";

and the handler pages with that bare tag, after resolving the SLO it was asked about — `src/ops/list_service_level_objective_exclusion_windows.rs:174-191`:

            store.exclusion_windows_of(&input.id).to_vec()
        };
        ...
        let (exclusion_windows, next_token) = crate::state::page_of(
            windows,
            crate::state::EXCLUSION_WINDOWS_TOKEN_TAG,
            input.next_token.as_deref(),
            page_size,
        )

Test coverage: `tests/all/exclusion_window_reads.rs:137-207` (`either_id_form_answers_the_same_page`) pins that a token from a by-name read resumes the SAME SLO read by ARN; `tests/all/exclusion_window_reads.rs:212-219` (`a_read_reports_only_the_set_of_the_slo_it_names`) pins per-SLO isolation of the SETS but never replays one SLO's token against another, so the cross-SLO case is untested.

## Steps to Reproduce

1. Create two SLOs, `slo-a` and `slo-b` (`POST /slo`).
2. Add 4 exclusion windows to `slo-a` and 4 different ones to `slo-b` (`PATCH /exclusion-windows`).
3. Call `GET /slo/slo-a/exclusion-windows?MaxResults=2` and keep the returned `NextToken` (it encodes the offset 2 under the tag `exclusion-windows/1/`).
4. Call `GET /slo/slo-b/exclusion-windows?MaxResults=2&NextToken=<that token>`.
5. Expected: a 400 `ValidationException` — the token was not issued for this SLO's projection, which is what the crate's instrumentation cursors guarantee by binding their subject into the tag.
6. Actual: HTTP 200 reporting `slo-b`'s 3rd and 4th windows, silently skipping its first two.
