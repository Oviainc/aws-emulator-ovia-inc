Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 1d8e7a96d86692d82e19cf4e2c81cc5cb0602adc

# ExportComponents' doc says it refuses a token "this operation never issued", but it serves the token ListComponents issued

_Filed: 2026-11-09T10:29:10Z_
_Operation: ExportComponents | Class: pagination_

> _Note: this issue was identified against the code base at commit `1d8e7a96d86692d82e19cf4e2c81cc5cb0602adc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Five of this crate's seven paginated handlers state in their module doc that the refusal rule for `nextToken` is keyed to the OPERATION that issued the token. The implementation deliberately keys it to the COLLECTION instead: `page_of` reads the collection off the record kind (`PagedRecord::COLLECTION`), so `ExportComponents` honours a token `ListComponents` issued over the same app environment's components, and vice versa. The state module writes that design down explicitly, the crate's own integration suite pins it (a list-issued token presented to the export of the same collection must answer 200 and resume at the named record), and `export_forms.rs` words the same rule correctly as "a token this SERVICE never issued". So the doc of `ExportComponents` (and of `ListComponents`, `ListForms`, `ExportThemes`) states a refusal rule its own handler does not implement: a reader of the doc expects a `ListComponents` token to be refused with `InvalidParameterException`, where the handler serves a page. (`ListCodegenJobs` is the one case where the "this operation" wording happens to be true, because codegen jobs are paged by their list alone.)

## Evidence

The claim, in the handler under review:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/export_components.rs:10-11
    //! `nextToken` resumes the same creation order. A token this operation never issued for
    //! that app environment is refused with the modelled `InvalidParameterException`.

The same wording in its siblings:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/list_forms.rs:7-8
    //! at; a token this operation never issued for that app environment is refused with the
    //! modelled `InvalidParameterException`.
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/export_themes.rs:9-11
    //! `nextToken` resumes the same creation order. A token this operation never issued for
    //! that app environment — one issued over another of this service's collections included —
    //! is refused with the modelled `InvalidParameterException`.

What the code actually does — the collection, not the operation, is the discriminator:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1606-1609
    /// collection comes off the record kind rather than from the handler, so the
    /// operations that page one collection — its list and its export alike — all
    /// honour each other's tokens and no operation can mint a token under another
    /// collection's name.
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/export_components.rs:171-178
        let page = crate::state::page_of(
            store.components_in(&input.app_id, &input.environment_name),
            &key,
            store.cursor_seq,
            input.next_token.as_deref(),
            crate::state::EXPORT_PAGE_LIMIT,
            crate::state::StoredComponent::to_component,
        )

The sibling that words it right:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/export_forms.rs:9-10
    //! `nextToken` resumes the same creation order. A token this service never issued for
    //! that app environment is refused with the modelled `InvalidParameterException`.

The existing test that pins the behaviour the doc denies (the token is taken from the collection's LIST and presented to every paginated operation, the export included):

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/page_token_collection_scope.rs:382-402
            if *issuer == collection {
                // The operation pages the records the token was issued over, so it resumes
                // exactly where the token points: at the records after the first one.
                ...
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: a token of the collection this operation pages must be \
                     served, got {} with {:?}",

Test coverage today: tests/all/page_token_collection_scope.rs (all four collections against all seven paginated operations) and tests/all/shared_page_walk.rs cover the walk; correcting the four doc sentences leaves every assertion unchanged.

## Steps to Reproduce

1. Create two components in app d1234abcd / environment staging (POST /app/d1234abcd/environment/staging/components).
2. GET /app/d1234abcd/environment/staging/components?maxResults=1 and keep the `nextToken` it returns — a token ExportComponents never issued.
3. GET /export/app/d1234abcd/environment/staging/components?nextToken=<that token>.
4. Observed: HTTP 200 with the second component, because `page_of` keys the token to the COLLECTION (state.rs:1606-1609).
5. Expected per export_components.rs:10-11 ("A token this operation never issued ... is refused with the modelled InvalidParameterException"): HTTP 400. The doc states a rule the handler does not implement; export_forms.rs:9-10 states the implemented rule ("a token this service never issued").
