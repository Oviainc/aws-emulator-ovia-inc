Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 68144f2b1a9e2dba602611483c97527f7042e8b8

# A ListServiceLevelObjectives NextToken carries no record of the filters it was issued under, so resuming it with a different filter silently drops matching SLOs

_Filed: 2026-10-28T22:19:35Z_
_Operation: ListServiceLevelObjectives | Class: pagination_

> _Note: this issue was identified against the code base at commit `68144f2b1a9e2dba602611483c97527f7042e8b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListServiceLevelObjectives` selects its ordered set from five request members (`KeyAttributes`, `OperationName`, `DependencyConfig`, `MetricSource`, `MetricSourceTypes` — see `matches_filters`), but its page token is minted and resolved under the bare operation tag `"slos/1/"` plus the last SLO name, with nothing of that selection bound into it. A token handed back by one filtered listing is therefore accepted by a listing with completely different filters and honoured as a cursor into it: the resumed call reports only the SLOs whose name sorts after the cursor, and every SLO that matches the new filter but sorts at or before it is silently omitted — the caller is told the listing has ended (no further `NextToken`) while records it asked for were never reported.

Every other paged read of this crate binds its selection into the token tag precisely to prevent that, including `ListInstrumentationConfigurations`, whose cursor is a stored KEY exactly like this one — so the key-vs-offset difference is not the crate's reason for omitting the binding here. The crate states the rule once in `state::subject_token_tag` and this is the only read whose request-selected set is paged without it.

## Evidence

THE SET IS A FUNCTION OF FIVE REQUEST MEMBERS — services/service-com-amazonaws-applicationsignals-applicationsignals/src/ops/list_service_level_objectives.rs:231-266:
```
fn matches_filters(
    slo: &ServiceLevelObjective,
    input: &ListServiceLevelObjectivesInput,
) -> bool {
    let entity = crate::state::entity_of(slo);

    if let Some(filter) = input.key_attributes.as_ref() {
...
    if let Some(kinds) = input.metric_source_types.as_ref() {
```

THE TOKEN BINDS NONE OF THEM — src/ops/list_service_level_objectives.rs:190-200:
```
        let cursor = match input.next_token.as_deref() {
            None => None,
            Some(token) => match crate::state::decode_next_token(token) {
                Some(cursor) => Some(cursor),
                None => {
                    return Err(crate::scaffolded::Scaffolded::validation(
                        crate::state::UNISSUED_NEXT_TOKEN,
                    ))
                }
            },
        };
```
and what that tag is — src/state.rs:177 `const NEXT_TOKEN_TAG: &str = "slos/1/";` with src/state.rs:1313-1315 / 1321-1323:
```
pub fn encode_next_token(last_name: &str) -> String {
    encode_cursor(NEXT_TOKEN_TAG, last_name)
}
...
pub fn decode_next_token(token: &str) -> Option<String> {
    decode_cursor(NEXT_TOKEN_TAG, token)
}
```

THE CRATE'S OWN RULE, STATED ONCE — src/state.rs:1439-1447 (`subject_token_tag`):
```
/// A [`page_of`] cursor is an offset within ONE ordered set, so a token issued over another
/// selection would silently skip that selection's first entries if it were honoured. Binding
/// the selection into the tag makes such a token fail to decode, so it is refused like any
/// other token this service did not issue — the rule the scoped instrumentation reads already
/// hold ([`instrumentation_token_tag`]), stated once here for every read whose set the request
/// picks out. Every request member the projection is a function of belongs in `subject`: the
/// subject the request names, the filters that narrow it ([`filtered_window_token_tag`]) and
/// the period it covers ([`window_fields`])
```

SIBLINGS THAT APPLY IT — src/ops/list_service_states.rs:246-251 binds the attribute filters (`filtered_window_token_tag`), src/ops/list_service_dependencies.rs:210-215 and src/ops/list_service_operations.rs:207-212 bind the entity and window (`entity_token_tag`), src/ops/list_service_level_objective_exclusion_windows.rs:197-200 binds the SLO (`subject_token_tag`), and — with a KEY cursor, like this read — src/ops/list_instrumentation_configurations.rs:206-209:
```
        // The token is resolved under the tag of the scope that issued it, so a token from
        // another service, environment or instrumentation type is refused rather than
        // honoured as a cursor into this scope.
        let token_tag = crate::state::instrumentation_token_tag(&scope);
```
whose state-side cursor is documented at src/state.rs:3996-3998 as "a stored KEY rather than an offset".

TEST COVERAGE — tests/all/paged_cursor_selection_binding.rs:264-466 asserts the refusal for ListAuditFindings, ListServiceOperations, ListServiceDependencies, ListServiceDependents, ListEntityEvents, ListServices and ListServiceStates, and ListServiceLevelObjectives is absent from that table; tests/all/slo_lifecycle.rs:849-861 exercises every filter WITHOUT a token, and tests/all/page_bounds.rs:74-85 pages /slos with no filter at all — so no existing assertion pins the cross-filter replay either way.

## Steps to Reproduce

1. POST /slo three times to store "slo-1" (SliConfig KeyAttributes {Type:Service, Name:alpha, Environment:prod}), "slo-2" and "slo-3" (both KeyAttributes {Type:Service, Name:beta, Environment:prod}).
2. POST /slos?MaxResults=1 with body {"KeyAttributes":{"Type":"Service","Name":"beta","Environment":"prod"}} — the page reports "slo-2" and hands back a NextToken (hex of "slos/1/slo-2").
3. Replay that token against a DIFFERENT filter: POST /slos?MaxResults=1&NextToken=<token> with body {"KeyAttributes":{"Type":"Service","Name":"alpha","Environment":"prod"}}. The token is accepted and the answer is an empty SloSummaries with no NextToken, so "slo-1" — the only SLO matching the alpha filter, which sorts before the cursor — is never reported and the caller is told the listing ended.
4. Compare the sibling rule: issue a ListServiceStates token with no AttributeFilters and replay it with an AttributeFilters entry (tests/all/paged_cursor_selection_binding.rs:405-421) — that replay is refused with HTTP 400 ValidationException naming NextToken instead of silently skipping entries.
