Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the EventBridge Pipes (pipes) emulator

_Filed: 2026-10-21T21:27:58Z_
_Operation: service-com-amazonaws-pipes-pipes | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-pipes-pipes/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client can do anything with this service. Implement a first, small, stateful slice covering the lifecycle of the primary resource (a Pipe), wire protocol `restJson1`:

1. **CreatePipe** — `POST /v1/pipes/{Name}`. Required input: `Name` (@httpLabel, 1..64, `^[\.\-_A-Za-z0-9]+$`), `Source`, `Target`, `RoleArn`; optional `Description`, `DesiredState` (RUNNING|STOPPED), `SourceParameters`, `Enrichment`, `EnrichmentParameters`, `TargetParameters`, `Tags`, `LogConfiguration`, `KmsKeyIdentifier`. Must persist the whole record, mint `Arn` = `arn:aws:pipes:<region>:<account>:pipe/<Name>`, set `CreationTime`/`LastModifiedTime` to now, and return `Arn`, `Name`, `DesiredState` (default RUNNING when omitted), `CurrentState`. Creating a Name that already exists must return `ConflictException` (409).

2. **DescribePipe** — `GET /v1/pipes/{Name}`. Must read the persisted record back and return every stored field (`Arn`, `Name`, `Description`, `DesiredState`, `CurrentState`, `StateReason`, `Source`, `SourceParameters`, `Enrichment`, `EnrichmentParameters`, `Target`, `TargetParameters`, `RoleArn`, `Tags`, `CreationTime`, `LastModifiedTime`, `LogConfiguration`, `KmsKeyIdentifier`) exactly as created/updated. Unknown Name → `NotFoundException` (404).

3. **ListPipes** — `GET /v1/pipes`. Must enumerate the persisted pipes as `Pipes` (list<Pipe>) and honour the filters `NamePrefix`, `DesiredState`, `CurrentState`, `SourcePrefix`, `TargetPrefix` plus `Limit` (1..100) with an opaque `NextToken` round-trip. A newly created pipe must appear; a deleted one must not.

4. **StopPipe** / **StartPipe** — `POST /v1/pipes/{Name}/stop` and `/start`. Must mutate the stored pipe's `DesiredState` to STOPPED/RUNNING and its `CurrentState` (STOPPED/RUNNING is acceptable for a first slice; transitional STOPPING/STARTING optional), bump `LastModifiedTime`, and return `Arn`, `Name`, `DesiredState`, `CurrentState`, `CreationTime`, `LastModifiedTime` computed from the stored record. A later DescribePipe must observe the new state. Unknown Name → `NotFoundException` (404).

5. **DeletePipe** — `DELETE /v1/pipes/{Name}`. Must remove the pipe from the store and return the final `Arn`, `Name`, `DesiredState` = `DELETED`, `CurrentState` (DELETING/DELETED), `CreationTime`, `LastModifiedTime`. A subsequent DescribePipe/ListPipes must no longer see it; a second DeletePipe → `NotFoundException` (404).

NEVER STUBS: each of these must compute its response from the persisted store reached through `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the request back without storing, no minting an ARN that is never saved. Input constraints already modeled (name pattern/length, ARN patterns, `Limit` range) should produce `ValidationException` (400) rather than being silently accepted.

## State Model

In `src/state.rs`, the service state holds:

- `pipes: BTreeMap<String /* Name */, Pipe>` — ordered by name so ListPipes and its NextToken paging are deterministic.
- `Pipe { arn: String, name: String, description: Option<String>, desired_state: PipeState /* RUNNING|STOPPED|DELETED */, current_state: PipeState, state_reason: Option<String>, source: String, source_parameters: Option<PipeSourceParameters>, enrichment: Option<String>, enrichment_parameters: Option<PipeEnrichmentParameters>, target: String, target_parameters: Option<PipeTargetParameters>, role_arn: String, tags: BTreeMap<String,String>, creation_time: SystemTime, last_modified_time: SystemTime, log_configuration: Option<PipeLogConfiguration>, kms_key_identifier: Option<String> }`.
- Tags live on the `Pipe` record itself (keyed by the pipe ARN) so the later TagResource/UntagResource/ListTagsForResource slice can reuse them.

Out of scope for this first slice (stay 501, filed separately later): UpdatePipe, TagResource, UntagResource, ListTagsForResource, and any actual event polling/enrichment/target delivery — a Pipe in this emulator is a configuration record, not a running data plane.

## Evidence

`OPERATIONS.md` at the crate root lists all 10 bound operations with their routes, input/output members and errors (CreatePipe `POST /v1/pipes/{Name}`, DescribePipe `GET /v1/pipes/{Name}`, ListPipes `GET /v1/pipes` with @paginated(inputToken=NextToken, outputToken=NextToken, items=Pipes), StartPipe/StopPipe `POST /v1/pipes/{Name}/start|stop`, DeletePipe `DELETE /v1/pipes/{Name}`), and names `ConflictException (409)`, `NotFoundException (404)`, `ValidationException (400)` as the modeled failures used above. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-pipes-pipes/src/ops/` matches all 10 handler files (create_pipe.rs, describe_pipe.rs, list_pipes.rs, start_pipe.rs, stop_pipe.rs, delete_pipe.rs, update_pipe.rs, tag_resource.rs, untag_resource.rs, list_tags_for_resource.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501. README.md confirms the restJson1 wire protocol and the shared-engine state access pattern.

## Steps to Reproduce

1. CreatePipe Name=demo-pipe, Source=<SQS queue ARN or smk:// source>, Target=<target ARN>, RoleArn=arn:aws:iam::123456789012:role/pipes-role, Description='demo', Tags={env:test} → expect 200 with Arn containing `pipe/demo-pipe`, Name=demo-pipe, DesiredState=RUNNING, CurrentState set, CreationTime/LastModifiedTime populated
2. DescribePipe Name=demo-pipe → expect 200 echoing the persisted Source, Target, RoleArn, Description and Tags exactly as created (not empty, not defaults)
3. ListPipes (no filters) → expect demo-pipe in Pipes; ListPipes NamePrefix=demo → still present; ListPipes NamePrefix=other → empty
4. StopPipe Name=demo-pipe → expect 200; DescribePipe → DesiredState=STOPPED and CurrentState reflecting stopped
5. StartPipe Name=demo-pipe → expect 200; DescribePipe → DesiredState=RUNNING again, LastModifiedTime advanced
6. CreatePipe Name=demo-pipe again → expect ConflictException 409
7. DeletePipe Name=demo-pipe → expect 200 with DesiredState=DELETED; then DescribePipe Name=demo-pipe → NotFoundException 404 and ListPipes no longer contains it
8. DescribePipe Name=never-created → expect NotFoundException 404 (currently every call above returns HTTP 501 NotImplemented)
