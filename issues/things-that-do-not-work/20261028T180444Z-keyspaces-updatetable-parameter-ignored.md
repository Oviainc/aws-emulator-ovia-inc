Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 5cb591631d698c9098c00693287b08686d77b0d1

# UpdateTable validates then discards cdcSpecification.tags when the specification disables CDC, although the disabled stream still exists and still holds tags

_Filed: 2026-10-28T18:04:44Z_
_Operation: UpdateTable | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `5cb591631d698c9098c00693287b08686d77b0d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateTable`'s CDC block computes and validates the tag set a `cdcSpecification` applies to the table's stream for EVERY status (the handler says so explicitly), but on the disabling branch it calls `disable_stream` and stores nothing: the computed `stream_tags` is dropped and the stream ARN that `disable_stream` returns is discarded. The comment that justifies this states the stream is gone ("A stream that is gone carries no tags either") — which is false in this crate. Disabling CDC does not remove the stream: this crate's own suites assert that after a disable the stream is still readable, is still reported by `GetTable` as the table's `latestStreamArn`/`cdcSpecification`, that its tag set stays with it, and that a `TagResource` write to that very stream ARN lands and is reported by `ListTagsForResource`. So a documented input member (`CdcSpecification$tags`, "The tags (key-value pairs) that you want to apply to the stream") is checked for legality and then silently dropped for a resource that exists and demonstrably takes tag writes through the sibling tagging path. A second symptom of the same defect: the base the tags would be merged onto is deliberately emptied for this branch (`live.as_ref().filter(|_| cdc.enabled)`), so the disabling path never even looks at the tags the stream already holds.

## Evidence

Handler, the branch that drops them — services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/update_table.rs:523-533:
```
                // A table with no live stream has nothing to disable, which is why that
                // answers nothing: asking for CDC a table does not have is the state it
                // is already in. A stream that is gone carries no tags either, so the
                // specification's tags are validated above and stored nowhere.
                false => {
                    stream_store.disable_stream(
                        &region,
                        &input.keyspace_name,
                        &input.table_name,
                    );
                }
```
The same handler states it validates them whatever the status asks for — update_table.rs:468-480:
```
            // The tags the specification applies to the stream, under the same
            // per-resource rules wherever they are supplied — checked whatever the status
            // asks for, because a reserved key or an oversized list is the same bad input
            // either way — onto the tags that stream already holds when it is one that
            // already exists.
            let mut stream_tags = live
                .as_ref()
                .filter(|_| cdc.enabled)
```
The crate's own test says the premise is wrong — services/service-com-amazonaws-keyspaces-keyspacesservice/tests/all/cdc_stream_tag_lifetime.rs:324-326:
```
/// The same rule from the other side: a stream the control plane has NOT forgotten keeps
/// answering for its tags through a disable — `DISABLED` is a status a caller can still
/// read, so the tag set stays with it — and a write to those tags still lands.
```
and tests/all/cdc_streams.rs:629-630:
```
    // Turning CDC off stops the stream and settles it into DISABLED; the stream stays
    // readable, and the table still reports it as its latest.
```
Model: `CdcSpecification$tags` — "The tags (key-value pairs) that you want to apply to the stream." (smithy_query shape="com.amazonaws.keyspaces#CdcSpecification"), with no restriction to a particular `status` value.
Test coverage: tests/all/cdc_stream_tag_lifetime.rs covers tag lifetime on delete and on disable, and tests/all/cdc_streams.rs covers the enable/disable/re-enable walk — neither exercises a disabling `cdcSpecification` that also carries `tags`, which is why the drop is unnoticed.

## Steps to Reproduce

1. CreateKeyspace(keyspaceName="ks")
2. CreateTable(keyspaceName="ks", tableName="tbl", schemaDefinition=<minimal>, cdcSpecification={"status":"ENABLED","tags":[{"key":"stream_tag","value":"v"}]}) and wait for the table to settle
3. GetTable(keyspaceName="ks", tableName="tbl") and note `latestStreamArn` (call it S)
4. UpdateTable(keyspaceName="ks", tableName="tbl", cdcSpecification={"status":"DISABLED","tags":[{"key":"disable_tag","value":"v"}]}) -> HTTP 200
5. ListTagsForResource(resourceArn=S) -> only `stream_tag` is reported; `disable_tag` was validated and dropped, although GetTable still reports S as the table's latestStreamArn and GetStream(S) still answers
6. TagResource(resourceArn=S, tags=[{"key":"disable_tag","value":"v"}]) -> HTTP 200 and ListTagsForResource(S) now reports it, proving the stream is a live taggable resource the UpdateTable path refused to write
