Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 2271003192907dc435917d10fce42c387f42c2e4

# The cursor-and-page-bound rule is written out in full in both paginated list handlers

_Filed: 2026-10-29T02:36:33Z_
_Operation: ListInstanceTypes | Class: maintainability_

> _Note: this issue was identified against the code base at commit `2271003192907dc435917d10fce42c387f42c2e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate factored the pagination *primitives* into one place (`state::encode_next_token`, `state::decode_next_token`, `state::invalid_next_token`), but the three control-flow rules built on top of them are written out a second time, handler by handler:

1. the default page size derived from `MaxResults` (`input.max_results.map_or(<DEFAULT>, |n| n as usize)`),
2. the "decode the token, resolve it to a resume point, otherwise refuse with `invalid_next_token`" match, and
3. the "one past the page mints the token" loop (`if items.len() == page_size { next_token = Some(encode_next_token(<key>)); break; }`).

`ListInstanceTypes` and `ListWorkspaceInstances` each carry a full copy of all three; the only differences are the resolver (`instance_type_index` vs `instance_resume_point`), the error enum, and the key the token is minted from. Because the rule lives in two places, the two copies can drift on the page boundary, on whether a token one past the page is minted, and on whether an unresolvable token is refused — and this crate already shows that drift is real: `ListRegions` carries a third, differently worded copy of the bad-token refusal (`src/ops/list_regions.rs:174`-182, hand-rolled instead of `invalid_next_token()`; that copy's wording is already tracked by the open issue `20261028T205203Z-workspaces-instances-listregions-error-response`).

The fix is to state the rule once — a `resume_point(token, resolve)` + `paginate(items, page_size, key)` pair beside `encode_next_token`/`decode_next_token` in `state.rs`, which is where this crate already keeps its one token encoding — and have both handlers call it. No response changes: both handlers already produce identical page boundaries, identical tokens and the same refusal, so every existing pagination test must keep passing byte for byte.

## Evidence

Copy 1 — `src/ops/list_instance_types.rs:182`-205, 213-227:
```rust
        let resume_from = match input.next_token.as_deref() {
            None => 0,
            Some(token) => {
                let decoded = crate::state::decode_next_token(token)
                    .and_then(|name| crate::state::instance_type_index(&name));
                match decoded {
                    Some(index) => index,
                    None => {
                        return Err(ListInstanceTypesErrorModeled::ValidationException(
                            crate::state::invalid_next_token(),
                        )
                        .into());
                    }
                }
            }
        };
        // `MaxResults` is modelled `range 1..600`, already enforced at the boundary; a
        // caller that sends none gets that modelled maximum as the page size.
        let page_size = input
            .max_results
            .map_or(crate::state::DEFAULT_INSTANCE_TYPES_MAX_RESULTS, |n| {
                n as usize
            });
...
            if instance_types.len() == page_size {
                // One past the page: its name is what the next page resumes at.
                next_token = Some(crate::state::encode_next_token(&spec.instance_type));
                break;
            }
```

Copy 2 — `src/ops/list_workspace_instances.rs:158`-162, 179-194, 219-234:
```rust
        // `MaxResults` is modelled `range 1..25`, already enforced at the boundary; a
        // caller that sends none gets that modelled maximum as the page size.
        let page_size = input
            .max_results
            .map_or(crate::state::DEFAULT_MAX_RESULTS, |n| n as usize);
...
        let resume_from = match input.next_token.as_deref() {
            None => None,
            Some(token) => {
                let decoded = crate::state::decode_next_token(token)
                    .and_then(|id| state.instance_resume_point(&region, &id));
                match decoded {
                    Some(id) => Some(id),
                    None => {
                        return Err(ListWorkspaceInstancesErrorModeled::ValidationException(
                            crate::state::invalid_next_token(),
                        )
                        .into());
                    }
                }
            }
        };
...
            if workspace_instances.len() == page_size {
                // One past the page: its id is what the next page resumes at.
                next_token = Some(crate::state::encode_next_token(&record.id));
                break;
            }
```

The crate itself states the rule should live once — `src/state.rs:2357`: "Both paginated operations that mint a token use this, so the scheme lives in one place." That holds for the ENCODING only; the cursor/page-bound logic around it is duplicated.

Drift already present in a third copy — `src/ops/list_regions.rs:174`-181:
```rust
        if input.next_token.is_some() {
            const WHY: &str = "NextToken is not a token this service issued: ListRegions \
                               reports every supported Region in one response and returns \
                               no pagination token";
            return Err(ListRegionsErrorModeled::ValidationException(
                crate::state::field_validation_failed("NextToken", WHY),
            )
            .into());
        }
```

Test coverage that must stay unchanged: `tests/all/list_pagination_resume_points.rs` (`a_token_resumes_at_the_record_it_resolves_to`, `every_token_this_service_mints_resolves_and_pages_the_whole_list_once`, `a_token_that_resolves_to_no_record_of_this_region_is_refused`, `a_resume_point_is_scoped_to_the_region_whose_list_issued_it`), `tests/all/workspace_instance_lifecycle.rs:400` (`pagination_round_trips_the_whole_set_at_every_page_size`) and `:1022` (`a_next_token_this_service_did_not_issue_is_refused`), `tests/all/instance_type_catalog.rs:344` (`paging_walks_the_same_catalog_at_every_page_size`) and `:417` (`a_request_outside_the_modelled_pagination_contract_is_refused`).

## Steps to Reproduce

1. Open `services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/ops/list_instance_types.rs` and read lines 182-227: the page-size default, the token decode/resolve/refuse match and the one-past-the-page token mint.
2. Open `services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/ops/list_workspace_instances.rs` and read lines 158-234: the same three rules again, differing only in the resolver, the error enum and the key the token is minted from.
3. Note `src/state.rs:2357` claims the pagination scheme 'lives in one place', which is true of the hex encoding only — the cursor and page-bound rules are not shared.
4. Note the third, divergent copy of the bad-token refusal in `src/ops/list_regions.rs:174`-181, which already words the same refusal differently, showing the copies drift.
5. Change the page boundary in one handler (for example `>=` instead of `==`) and observe that nothing in the other handler is affected: the rule has no single owner.
