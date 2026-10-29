Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c22a521c3662ecce02874628fef1ed26c367e273

# UpdateTable refuses a cdcSpecification that names no viewType, treating the DEFAULTED view type as an attempted change

_Filed: 2026-10-29T04:06:23Z_
_Operation: UpdateTable | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c22a521c3662ecce02874628fef1ed26c367e273`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateTable`'s CDC branch compares the table's live stream view type against `ResolvedCdc::view_type` — but that field is the DEFAULTED value: `resolve_cdc` fills it with `DEFAULT_CDC_VIEW_TYPE` (`NEW_AND_OLD_IMAGES`) whenever the request's `cdcSpecification` omits `viewType`, which the model allows (only `status` is `@required` on `CdcSpecification`).

So for any table whose live CDC stream publishes a NON-default view type (`KEYS_ONLY`, `NEW_IMAGE`, `OLD_IMAGE`), a request that names no `viewType` at all is refused with 400 `ValidationException` claiming the caller tried to change the selection to `NEW_AND_OLD_IMAGES`. The handler cannot distinguish "the caller asked for NEW_AND_OLD_IMAGES" from "the caller asked for nothing", and the refusal message names a value the request never carried.

The crate's own texts state the narrower rule the code was meant to implement: the comment directly above the check says a request "naming a different view type" is refused, and the comment where `cdc` is resolved says the resolved view type is the one "(for one being created)" — i.e. it describes a stream being minted, not an existing one. The state helper's own doc says the field is "defaulted when the specification omits it".

Consequence: the whole re-assert path is unreachable for three of the four modelled view types. In particular `cdcSpecification.tags` — the only way `UpdateTable` applies tags to an existing stream — can never be used on a `KEYS_ONLY`/`NEW_IMAGE`/`OLD_IMAGE` stream unless the caller happens to repeat the exact stored view type, and a plain idempotent re-assert of `{status: ENABLED}` fails too (the same holds for `{status: ENABLING}`, which `resolve_cdc` also treats as enabled).

## Evidence

The defaulting, `services/service-com-amazonaws-keyspaces-keyspacesservice/src/state.rs:2183-2187` (the field doc) and `:2209-2212` (the code):
```
    /// The `ViewType` that stream publishes, defaulted when the specification omits it.
    /// It decides which images a record carries, which is fixed when the stream is
    /// created ("After you create the stream, you can't make changes to this
    /// selection"), so it says nothing about a table whose CDC is being turned off.
    pub view_type: String,
...
        view_type: specification
            .view_type
            .clone()
            .unwrap_or_else(|| DEFAULT_CDC_VIEW_TYPE.to_string()),
```

The rule the handler states it implements, `src/ops/update_table.rs:454-467`:
```
            // "The view type specifies the changes Amazon Keyspaces records for each
            // changed row in the stream. After you create the stream, you can't make
            // changes to this selection." A request naming a different view type for a
            // stream that already exists is refused, rather than silently ignored or
            // applied to records the consumer is already reading.
            if let Some((_, view_type)) = live.as_ref().filter(|_| cdc.enabled) {
                if view_type != &cdc.view_type {
                    return Err(invalid(format!(
                        "cdcSpecification.viewType is '{}' for the stream of table \
                         '{}.{}' and cannot be changed to '{}' while that stream exists",
                        view_type, input.keyspace_name, input.table_name, cdc.view_type
                    )));
                }
            }
```
Nothing between `resolve_cdc` and this check records whether the caller supplied `viewType`; the same file says the resolved value describes a stream being created — `src/ops/update_table.rs:311-313`:
```
        // "The CDC stream settings of the table": whether the table captures its changes
        // to a stream from now on, and (for one being created) which images it publishes.
        let cdc = crate::state::resolve_cdc(input.cdc_specification.as_ref());
```

The model makes `viewType` optional with a default — `smithy_query(query="shape", shape="com.amazonaws.keyspaces#CdcSpecification")`: `status` is the only `required: true` member, and `viewType`'s doc reads "`NEW_AND_OLD_IMAGES` - both versions of the row, before and after the change. This is the default."

Test coverage leaves this case open, so a fix changes no existing expectation. `tests/all/cdc_streams.rs:584-600` pins only the EXPLICIT different-view-type refusal:
```
    // "After you create the stream, you can't make changes to this selection": another
    // view type for the same stream is the modelled refusal, and it changes nothing.
    assert_failure(
        "view_type_change",
        emulator
            .keyspaces(
                "UpdateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "cdcSpecification": {"status": "ENABLED", "viewType": "OLD_IMAGE"},
                }),
            )
            .await,
        400,
        "ValidationException",
    );
```
and the tags-on-update test only ever builds streams with NO `viewType` — so the default always matches and the bug is invisible there, `tests/all/cdc_specification_tags_on_update.rs:126-129`:
```
                    "cdcSpecification": {
                        "status": "ENABLED",
                        "tags": [{"key": STREAM_TAG_KEY, "value": STREAM_TAG_CREATED}],
                    },
```

## Steps to Reproduce

1. Start the emulator with both the Keyspaces and KeyspacesStreams services mounted (as tests/all/cdc_streams.rs does)
2. CreateKeyspace {"keyspaceName":"cdc_view_ks"}
3. CreateTable {"keyspaceName":"cdc_view_ks","tableName":"tbl","schemaDefinition":<minimal>,"cdcSpecification":{"status":"ENABLED","viewType":"KEYS_ONLY"}}
4. Wait out the transient spans, then GetTable — cdcSpecification.viewType is KEYS_ONLY and latestStreamArn names the stream
5. UpdateTable {"keyspaceName":"cdc_view_ks","tableName":"tbl","cdcSpecification":{"status":"ENABLED","tags":[{"key":"env","value":"prod"}]}} — the request names NO viewType
6. Expected: 200, and ListTagsForResource on the stream ARN reports env=prod (the rule tests/all/cdc_specification_tags_on_update.rs pins for a default-view-type stream). Actual: 400 ValidationException "cdcSpecification.viewType is 'KEYS_ONLY' for the stream of table 'cdc_view_ks.tbl' and cannot be changed to 'NEW_AND_OLD_IMAGES' while that stream exists", and the stream keeps its old tags
7. Repeat with a bare {"status":"ENABLED"} (and with {"status":"ENABLING"}) — both are refused the same way, so CDC cannot be re-asserted on the table at all without repeating viewType:KEYS_ONLY
