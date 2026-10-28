Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 31ee5846b7ac7b9895607a192ecc1e91c8df8e5f

# The exclusion-window page cursor is a bare OFFSET into a stored, shiftable Vec, so a removal between two pages drops a window from the listing the store still holds

_Filed: 2026-10-28T20:20:11Z_
_Operation: ListServiceLevelObjectiveExclusionWindows | Class: pagination_

> _Note: this issue was identified against the code base at commit `31ee5846b7ac7b9895607a192ecc1e91c8df8e5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListServiceLevelObjectiveExclusionWindows` pages one SLO's STORED window set (`state.exclusion_windows: BTreeMap<String, Vec<ExclusionWindow>>`) through `crate::state::page_of`, whose cursor is a bare decimal OFFSET (`cursor.parse::<usize>()` / `items.into_iter().skip(offset)`).

The store field's own doc claims the opposite property for that cursor: "The order is the order the windows were added in, which is what makes a `ListServiceLevelObjectiveExclusionWindows` page cursor a stable boundary." An offset is only a stable boundary while the Vec does not shift, and the one writer of that Vec shifts it: `update_exclusion_windows` does `windows.remove(at)` on a `Vec`, so every window after the removed one moves one position down. A caller that pages the set while a `BatchUpdateExclusionWindows` removal lands between two pages is therefore served a set that is missing a window the store still holds — the window that slid into the slot the previous page already consumed is never reported, and the caller cannot tell.

The justification `page_of` gives for using an offset does not hold for this read: "The projections the time-ranged topology reads page over are DERIVED (they are recomputed from the stored SLOs on every call) rather than stored, so a cursor into them is an offset into their deterministic order, not a key of a stored record." An exclusion-window set is not derived — it is the stored record itself. The crate's other two pagers, both over stored collections, avoid exactly this with a KEY cursor and say so: `page_instrumentation_configurations` ("The cursor is a stored KEY rather than an offset, so a configuration created or expired between two pages cannot make the next page skip or repeat an entry") and `State::page` for the SLO list ("the page boundary is a cursor on that order and paging over more SLOs than `MaxResults` terminates having visited each matching SLO exactly once"). This read needs that property and does not get it, although the window the cursor would key on is the one `same_exclusion_window` already treats as a window's identity.

## Evidence

THE RULE THE STORE STATES FOR THIS CURSOR — services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:243-255:
```
    /// The windows of one SLO are an ORDERED SET identified by configuration: a removal
    /// names a window by its configuration (`RemoveExclusionWindows`: "The window
    /// configuration must match an existing exclusion window"), so two windows with the same
    /// configuration would be one window named twice. The order is the order the windows
    /// were added in, which is what makes a `ListServiceLevelObjectiveExclusionWindows` page
    /// cursor a stable boundary.
...
    pub exclusion_windows: BTreeMap<String, Vec<ExclusionWindow>>,
```

THE CURSOR THAT BREAKS IT — src/state.rs:1396-1432 (`page_of`), whose stated justification is that the items are derived rather than stored:
```
/// The projections the time-ranged topology reads page over are DERIVED (they are
/// recomputed from the stored SLOs on every call) rather than stored, so a cursor into them
/// is an offset into their deterministic order, not a key of a stored record.
...
    let offset = match token {
        None => 0,
        Some(token) => decode_cursor(tag, token)
            .and_then(|cursor| cursor.parse::<usize>().ok())
            .ok_or_else(|| UNISSUED_NEXT_TOKEN.to_string())?,
    };
    let mut page: Vec<T> = items.into_iter().skip(offset).collect();
```

THE READ THAT USES IT ON A STORED VEC — src/ops/list_service_level_objective_exclusion_windows.rs:182 and 201-207:
```
            (slo_name, store.exclusion_windows_of(&input.id).to_vec())
...
        let (exclusion_windows, next_token) = crate::state::page_of(
            windows,
            &token_tag,
            input.next_token.as_deref(),
            page_size,
        )
        .map_err(refused)?;
```

THE WRITER THAT SHIFTS IT — src/state.rs:1888-1896 (`update_exclusion_windows`):
```
        for window in remove {
            let Some(at) = windows
                .iter()
                .position(|stored| same_exclusion_window(stored, window))
            else {
                return Err(ExclusionWindowFailure::UnmatchedRemoval);
            };
            windows.remove(at);
        }
```

SIBLING PAGERS OVER STORED COLLECTIONS THAT DO NOT HAVE THE DEFECT — src/state.rs:3992-3998:
```
    /// At most `page_size + 1` records are collected, so the response is bounded by the page
    /// rather than by the number of stored configurations. The cursor is a stored KEY rather
    /// than an offset, so a configuration created or expired between two pages cannot make
    /// the next page skip or repeat an entry.
```
and src/state.rs:463-470 (`State::page`, the SLO list): "the page boundary is a cursor on that order and paging over more SLOs than `MaxResults` terminates having visited each matching SLO exactly once" (src/ops/list_service_level_objectives.rs:161-164).

TEST COVERAGE: tests/all/exclusion_window_reads.rs and tests/all/exclusion_windows.rs drive the paged read and the batch writer, and tests/all/paged_cursor_subject_binding.rs pins that a token issued for one SLO is refused for another — but no test interleaves a removal with a page sequence, so nothing pins the current skip.

## Steps to Reproduce

1. Start the emulator with this crate mounted; POST /slo to create an SLO named `windowed` with a valid `SliConfig`.
2. PATCH /exclusion-windows with {"SloIds":["windowed"],"AddExclusionWindows":[w0,w1,w2,w3,w4]} where each wN is a distinct window configuration (different `Reason`, say), so the stored Vec is [w0,w1,w2,w3,w4].
3. GET /slo/windowed/exclusion-windows?MaxResults=2 — the page reports [w0,w1] and a NextToken whose payload is the offset 2.
4. PATCH /exclusion-windows with {"SloIds":["windowed"],"RemoveExclusionWindows":[w0]} — `windows.remove(at)` leaves the stored Vec as [w1,w2,w3,w4].
5. GET /slo/windowed/exclusion-windows?MaxResults=2&NextToken=<the token from step 3> — the page reports [w3,w4] and no further token: w2 is never served by the sequence, although GET /slo/windowed/exclusion-windows with no token still reports it.
6. Contrast with the instrumentation-configuration list, whose cursor is a stored key (src/state.rs:3995-3998): create three configurations in one scope, page with MaxResults=1, delete the first, and the sequence still serves every surviving record exactly once.
