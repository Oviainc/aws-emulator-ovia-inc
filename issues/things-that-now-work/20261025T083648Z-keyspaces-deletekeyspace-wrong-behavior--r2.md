Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a727425709f65cfb01a0ac76d7787005b46d11cb

# DeleteKeyspace forgets the keyspace's CDC streams but leaves their tag sets, so ListTagsForResource answers for a stream GetStream no longer knows

_Filed: 2026-10-25T08:36:48Z_
_Operation: DeleteKeyspace | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a727425709f65cfb01a0ac76d7787005b46d11cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteKeyspace` puts the keyspace and every table in it into `DELETING` and immediately calls `forget_streams_in_keyspace` on the shared CDC stream store, but it does not clear each table's `cdc_stream_tags`. The keyspace record (and therefore the tables and their `cdc_stream_tags`) survives for the whole `TRANSIENT_STATUS_SPAN` until `settle` removes it.

During that span `ListTagsForResource`/`TagResource`/`UntagResource` still resolve a stream ARN through `ComAmazonawsKeyspacesKeyspacesserviceState::resource_tags` → `Table::cdc_stream_tags` and answer 200 with that stream's tags, while `GetStream` in KeyspacesStreams answers 404 for the same ARN. That is precisely the drift the sibling `DeleteTable` prevents with an explicit clear, and precisely what `state.rs` documents as impossible.

## Evidence

DeleteKeyspace forgets the streams and stops there — services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/delete_keyspace.rs:280-286:
```
        // The tables are gone, so the CDC streams that followed them are: a stream
        // captures the changes of one table, and that table no longer exists.
        streams
            .lock()
            .forget_streams_in_keyspace(&region, &input.keyspace_name);

        Ok(DeleteKeyspaceOutput {})
```
The sibling DeleteTable does the extra step, and says why — services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/delete_table.rs:262-272:
```
            // A tag set belongs to a stream, so the tag sets of the streams just
            // forgotten go with them: nothing may answer for a stream ARN the stream
            // store no longer holds, not even in the span before `settle` removes the
            // table itself.
            if let Some(table) = store
                .keyspaces
                .get_mut(&input.keyspace_name)
                .and_then(|keyspace| keyspace.tables.get_mut(&input.table_name))
            {
                table.cdc_stream_tags.clear();
            }
```
And the state module states the invariant the DeleteKeyspace path breaks — services/service-com-amazonaws-keyspaces-keyspacesservice/src/state.rs:1723-1727:
```
    /// A tag set belongs to a stream, and the CDC stream store caps how many streams one
    /// table keeps ([`MAX_STREAMS_PER_TABLE`]) — so the pruning here is driven by the
    /// ARNs that store still holds, not by a second cap of its own. That is what keeps
    /// the two from drifting: `ListTagsForResource` can never answer for a stream
    /// `GetStream` no longer knows.
```
The lookup that still succeeds — state.rs:1655-1665:
```
            ResourceRef::Stream {
                keyspace_name,
                table_name,
                ..
            } => self
                .keyspaces
                .get(&keyspace_name)?
                .tables
                .get(&table_name)?
                .cdc_stream_tags
                .get(resource_arn),
```
The window is non-zero by design — state.rs:144-147: "deliberately NON-ZERO, so a caller that reads straight after the call still observes the transient status the model documents" / `pub const TRANSIENT_STATUS_SPAN: Duration = Duration::from_millis(500);`.

Existing coverage that shows both halves are observable: tests/all/cdc_streams.rs:752-766 lists a stream's tags through `ListTagsForResource`, and tests/all/cdc_streams.rs:919-930 shows `DeleteKeyspace` leaving no stream in the stream store — but no test reads a stream's tags after `DeleteKeyspace`, which is why the gap is unnoticed.

## Steps to Reproduce

1. CreateKeyspace "ks".
2. CreateTable "t" in "ks" with cdcSpecification {status: ENABLED, tags: [{key: stream_tag, value: v}]}; wait out the CREATING span.
3. GetTable "ks"."t" and record latestStreamArn.
4. DeleteKeyspace "ks" (returns 200; the keyspace is now DELETING for ~500ms).
5. Immediately call KeyspacesStreams GetStream with that streamArn: 404 ResourceNotFoundException (the stream is forgotten).
6. Immediately call Keyspaces ListTagsForResource with the same streamArn: 200 with tags [{key: stream_tag, value: v}] — tags for a stream the store no longer holds. Repeating the same two calls after a DeleteTable of the table instead yields 404 from both, which is the behaviour DeleteKeyspace should match.
