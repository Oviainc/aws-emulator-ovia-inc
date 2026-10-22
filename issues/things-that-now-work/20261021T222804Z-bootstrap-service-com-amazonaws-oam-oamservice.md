Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 85378260976efce74d968b045ae62c2fe2a03011

# Bootstrap the oam (CloudWatch Observability Access Manager) emulator

_Filed: 2026-10-21T22:28:04Z_
_Operation: service-com-amazonaws-oam-oamservice | Class: stub_

> _Note: this issue was identified against the code base at commit `85378260976efce74d968b045ae62c2fe2a03011`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 15 operations in `services/service-com-amazonaws-oam-oamservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, holding no state. No client workflow works at all.

Implement this FIRST SLICE — the sink/link lifecycle of OAM — statefully, so a realistic monitoring-account setup script runs end to end. Operations outside this list stay stubbed and will be filed as individual gap issues later.

### 1. CreateSink (`POST /CreateSink`, restJson1)
Input: `Name` (required, must match `^[a-zA-Z0-9_\.\-]{1,255}$`), `Tags`. Must allocate a NEW random `Id` (uuid) and build `Arn` = `arn:aws:oam:<region>:<account>:sink/<Id>`, persist the sink keyed by Id, and return `Arn`, `Id`, `Name`, `Tags` read back from what was stored. A second CreateSink with the same `Name` must fail with `ConflictException` (409) — AWS allows only one sink per account/region, so a second create with a DIFFERENT name must also fail with `ServiceQuotaExceededException`/`ConflictException` rather than silently creating a second sink.

### 2. GetSink (`POST /GetSink`)
Input: `Identifier` (sink ARN or Id, required), `IncludeTags`. Must look the sink up in state and return the stored `Arn`, `Id`, `Name`; include `Tags` only when `IncludeTags` is true. Unknown identifier -> `ResourceNotFoundException` (404). Must NOT echo the identifier back as a synthesized sink.

### 3. CreateLink (`POST /CreateLink`)
Input: `LabelTemplate` (required, 1..64), `ResourceTypes` (required, 1..50, each a modeled `ResourceType` enum value such as `AWS::CloudWatch::Metric`, `AWS::Logs::LogGroup`, `AWS::XRay::Trace`), `SinkIdentifier` (required), `Tags`, `LinkConfiguration`. Must resolve `SinkIdentifier` against stored sinks (404 `ResourceNotFoundException` if absent), reject an unmodeled resource type with `InvalidParameterException` (400), allocate a new link `Id` and `Arn` = `arn:aws:oam:<region>:<account>:link/<Id>`, resolve `Label` from `LabelTemplate` (substitute `$AccountName`/`$AccountEmail` variables with the source account identity), persist the link including its `SinkArn` back-reference, and return the stored `Arn`, `Id`, `Label`, `LabelTemplate`, `ResourceTypes`, `SinkArn`, `Tags`, `LinkConfiguration`. A second link from the same source account to the same sink must fail with `ConflictException` (409).

### 4. GetLink (`POST /GetLink`)
Input: `Identifier` (required), `IncludeTags`. Must return the persisted link exactly as created/updated: `Arn`, `Id`, `Label`, `LabelTemplate`, `ResourceTypes`, `SinkArn`, `LinkConfiguration`, and `Tags` when `IncludeTags` is true. Unknown identifier -> `ResourceNotFoundException` (404).

### 5. ListAttachedLinks (`POST /ListAttachedLinks`)
Input: `SinkIdentifier` (required), `MaxResults` (1..1000), `NextToken`. Must enumerate from state only the links whose stored `SinkArn` matches the resolved sink, returning `Items` of `ListAttachedLinksItem` (`Label`, `LinkArn`, `ResourceTypes`). Must honour `MaxResults` by truncating and emitting an opaque `NextToken` that resumes the enumeration (and omit `NextToken` on the final page). Unknown sink -> `ResourceNotFoundException` (404). `Items` is `@required`, so an empty list must be `[]`, never absent.

### 6. DeleteLink then DeleteSink (`POST /DeleteLink`, `POST /DeleteSink`)
Input: `Identifier` (required). Each must remove the entity from state and return an empty 200 body. A follow-up GetLink/GetSink must then return `ResourceNotFoundException` (404), and ListAttachedLinks must no longer include the deleted link. DeleteSink on a sink that still has attached links must fail with `ConflictException` (409) (its modeled error), not delete the sink and orphan the links.

Every one of these must compute its response from persisted state — no canned responses, no echoing the input back, no random ID returned without storing it. Validation (the required members, the identifier pattern `^[a-zA-Z0-9][a-zA-Z0-9_:\.\-\/]{0,2047}$`, `LabelTemplate` length, `ResourceTypes` cardinality) must be enforced with the modeled `MissingRequiredParameterException` / `InvalidParameterException` (both HTTP 400).

## State Model
Add to `ComAmazonawsOamOamserviceState` in `src/state.rs` (reached via `crate::state::service_state(&*self.engine)`, mutated under `state.lock()`):

- `sinks: HashMap<String /* sink Id */, Sink>` where `Sink { id: String, name: String, arn: String, tags: HashMap<String,String>, policy: Option<String>, created_at: SystemTime }`
- `links: HashMap<String /* link Id */, Link>` where `Link { id: String, arn: String, label: String, label_template: String, resource_types: Vec<String>, sink_arn: String, sink_id: String, link_configuration: Option<LinkConfiguration>, tags: HashMap<String,String>, created_at: SystemTime }`
- `sink_name_index: HashMap<String /* Name */, String /* sink Id */>` so CreateSink can detect the duplicate-name `ConflictException` and GetSink/CreateLink can resolve an `Identifier` given as a name, Id or ARN
- `links_by_sink: HashMap<String /* sink Arn */, BTreeSet<String /* link Id */>>` giving ListAttachedLinks a deterministic (sorted) enumeration order for stable pagination and giving DeleteSink its "still has links" conflict check

Identifier resolution is shared by every op above: accept a bare Id, a `Name` (sinks), or a full ARN, and map it to the stored entity. Tag storage lives on `Sink`/`Link` so the later Tag/Untag/ListTagsForResource slice can read the same maps.

Done = a client script does CreateSink -> GetSink (fields match the create) -> CreateLink -> GetLink (fields match) -> ListAttachedLinks (contains exactly the created link) -> DeleteLink -> ListAttachedLinks (empty `Items`) -> DeleteSink -> GetSink (404), with every response built from the stored state.

## Evidence

`services/service-com-amazonaws-oam-oamservice/OPERATIONS.md` lists all 15 operations with their routes, required members, output members and modeled errors: CreateSink (`Name` required, outputs `Arn`/`Id`/`Name`/`Tags`, errors ConflictException 409 / ServiceQuotaExceededException 429), GetSink (@readonly, `Identifier` required, `IncludeTags`, ResourceNotFoundException 404), CreateLink (`LabelTemplate`/`ResourceTypes`/`SinkIdentifier` required, outputs `Arn`/`Id`/`Label`/`SinkArn`/`LinkConfiguration`), GetLink (@readonly), ListAttachedLinks (@paginated inputToken=NextToken outputToken=NextToken items=Items, `SinkIdentifier` required, `MaxResults` 1..1000, `Items` @required), DeleteLink and DeleteSink (`Identifier` required, no output members, ResourceNotFoundException 404; DeleteSink additionally ConflictException 409). Protocol is restJson1, signing name `oam`, ARN namespace `oam`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-oam-oamservice/src/ops/` matches all 15 handler files (create_sink.rs, get_sink.rs, create_link.rs, get_link.rs, list_attached_links.rs, delete_link.rs, delete_sink.rs, ...), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` = HTTP 501. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. aws oam create-sink --name demo-sink  # expect 200 with Arn/Id/Name; actual HTTP 501 NotImplemented
2. aws oam get-sink --identifier <Arn from step 1> --include-tags  # expect the same Arn/Id/Name persisted; actual 501
3. aws oam create-link --label-template '$AccountName' --resource-types AWS::CloudWatch::Metric AWS::Logs::LogGroup --sink-identifier <sink Arn>  # expect 200 with link Arn/Id/Label/SinkArn; actual 501
4. aws oam get-link --identifier <link Arn>  # expect the stored ResourceTypes and SinkArn echoed from state; actual 501
5. aws oam list-attached-links --sink-identifier <sink Arn> --max-results 1  # expect Items containing the link (Label, LinkArn, ResourceTypes); actual 501
6. aws oam delete-sink --identifier <sink Arn>  # expect ConflictException 409 while the link is still attached
7. aws oam delete-link --identifier <link Arn>  # expect empty 200; then list-attached-links returns Items: []
8. aws oam delete-sink --identifier <sink Arn>  # expect empty 200; then get-sink returns ResourceNotFoundException 404
