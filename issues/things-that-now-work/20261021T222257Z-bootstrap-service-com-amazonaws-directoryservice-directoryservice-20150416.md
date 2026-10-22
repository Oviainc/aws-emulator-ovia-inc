Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: aa53e03837fde54d44dc5b021f911ec63548de2f

# Bootstrap the Directory Service emulator

_Filed: 2026-10-21T22:22:57Z_
_Operation: service-com-amazonaws-directoryservice-directoryservice-20150416 | Class: missing_

> _Note: this issue was identified against the code base at commit `aa53e03837fde54d44dc5b021f911ec63548de2f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 80 operations in `services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/` is an unmodified scaffold that carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answers HTTP 501 `NotImplemented`, holding no state. Implement a FIRST minimal, stateful happy-path slice for the primary resource (the directory), wire protocol `awsJson1_1` (`POST /`, header `X-Amz-Target: DirectoryService_20150416.<OperationName>`).

### Happy path to implement (in order)

1. **CreateDirectory** — input `Name` (required, FQDN pattern), `Password` (required), `Size` (required, `Small`|`Large`), optional `ShortName`, `Description`, `VpcSettings`, `Tags`, `NetworkType`. Must mint a fresh directory id matching `^d-[0-9a-f]{10}$`, persist a Directory record (including the submitted `Tags`), and return that `DirectoryId`. Two CreateDirectory calls must yield two distinct ids and two distinct records.
2. **DescribeDirectories** — input optional `DirectoryIds`, `NextToken`, `Limit`; output `DirectoryDescriptions` (list of `DirectoryDescription`) + `NextToken`. Must return the directories actually stored, built from persisted state: `DirectoryId`, `Name`, `ShortName`, `Description`, `Size`, `Type` (`SimpleAD` for CreateDirectory, `MicrosoftAD` for CreateMicrosoftAD if later added), `Stage` (e.g. `Active`), `LaunchTime`, `StageLastUpdatedDateTime`, `Alias` (defaults to the directory id when no alias was created), `AccessUrl` (`http://<alias>.awsapps.com`), `SsoEnabled` (false by default), `VpcSettings`. Filtering by `DirectoryIds` must narrow the result; an unknown id must raise `EntityDoesNotExistException`. `Limit`/`NextToken` must page over stored records, and a malformed token must raise `InvalidNextTokenException`.
3. **CreateAlias** — input `DirectoryId` + `Alias` (both required); output echoes `DirectoryId` and `Alias`. Must store the alias on the directory record so a later DescribeDirectories reports it in `Alias` and in the derived `AccessUrl`. A second CreateAlias on the same directory, or an alias already used by another directory, must raise `EntityAlreadyExistsException`; an unknown `DirectoryId` must raise `EntityDoesNotExistException`.
4. **AddTagsToResource** — input `ResourceId` (a `d-` id) + `Tags` (required); empty output. Must merge the tags into the stored directory's tag map (same key overwrites), and raise `EntityDoesNotExistException` for an unknown `ResourceId`.
5. **ListTagsForResource** — input `ResourceId` (required), `NextToken`, `Limit`; output `Tags` + `NextToken`. Must return exactly the tags persisted by CreateDirectory/AddTagsToResource for that directory — not an empty list, not an echo of the request.
6. **DeleteDirectory** — input `DirectoryId` (required); output `DirectoryId`. Must remove the directory (and its tags/alias) from state and return the deleted id. A subsequent DescribeDirectories for that id must raise `EntityDoesNotExistException`, and a second DeleteDirectory of the same id must also raise `EntityDoesNotExistException`.

Every one of these must compute its response from persisted state held in the shared engine reached via `crate::state::service_state(&*self.engine)` — no canned responses, no echoed input, no minting an id that is never stored. All other operations stay 501 and are filed separately as gap issues.

## State Model

In `src/state.rs` (`ComAmazonawsDirectoryserviceDirectoryservice20150416State`):

- `directories: BTreeMap<String /* DirectoryId "d-xxxxxxxxxx" */, Directory>` — insertion-ordered/id-ordered so pagination is stable.
- `Directory { directory_id: String, name: String, short_name: Option<String>, description: Option<String>, size: Option<DirectorySize>, edition: Option<DirectoryEdition>, directory_type: DirectoryType, stage: DirectoryStage /* Active */, launch_time: SystemTime, stage_last_updated: SystemTime, alias: Option<String>, access_url: Option<String>, vpc_settings: Option<DirectoryVpcSettingsDescription>, network_type: Option<NetworkType>, sso_enabled: bool /* false */, desired_number_of_domain_controllers: Option<i32>, tags: BTreeMap<String, String>, password_hash_or_discarded: () /* Password must never be echoed back */ }`
- `aliases: BTreeMap<String /* alias */, String /* directory_id */>` — enforces global alias uniqueness for CreateAlias.
- `next_id_counter: u64` — source of the hex suffix so minted ids match `^d-[0-9a-f]{10}$` and are unique per process.

(Room for the next slices, not required now: `snapshots: BTreeMap<String /* s- id */, Snapshot>`, `trusts: BTreeMap<String /* t- id */, Trust>`.)

Done = a client script can run the six calls above end-to-end against the emulator and observe real state: the id returned by CreateDirectory appears in DescribeDirectories with the name/size it was created with, the alias and tags it later added are visible, and after DeleteDirectory the directory is gone.

## Evidence

OPERATIONS.md (crate root) documents all 80 operations and the awsJson1_1/X-Amz-Target wiring; the member lists above are taken verbatim from its `## CreateDirectory` (line 81), `## DescribeDirectories` (line 196, @paginated items=DirectoryDescriptions), `## CreateAlias` (line 63), `## AddTagsToResource` (line 45), `## ListTagsForResource` (line 395), and `## DeleteDirectory` (line 129) sections, including the declared errors (EntityDoesNotExistException, EntityAlreadyExistsException, InvalidNextTokenException). The `DirectoryDescription` field list is from smithy_query on that shape in `smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json`. Every file under `src/ops/` still returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` behind the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker documented in the crate README, i.e. HTTP 501 with zero stored state. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `ds` client at it.
2. CreateDirectory with Name=corp.example.com, Password=<valid>, Size=Small, Tags=[{Key=env,Value=dev}] — expect a DirectoryId matching ^d-[0-9a-f]{10}$; today HTTP 501 NotImplemented.
3. DescribeDirectories with no filter — expect one DirectoryDescription whose DirectoryId equals the one returned, Name=corp.example.com, Size=Small, Stage=Active, Alias defaulting to the directory id.
4. CreateAlias with that DirectoryId and Alias=corp-dev — expect the alias echoed, then DescribeDirectories must report Alias=corp-dev and AccessUrl=http://corp-dev.awsapps.com.
5. AddTagsToResource with ResourceId=<DirectoryId>, Tags=[{Key=owner,Value=team-a}], then ListTagsForResource — expect both env=dev and owner=team-a.
6. DescribeDirectories with DirectoryIds=[d-00000000ff] (never created) — expect EntityDoesNotExistException (HTTP 400), not 501 and not an empty success.
7. DeleteDirectory with that DirectoryId — expect the id returned; then DescribeDirectories with DirectoryIds=[<that id>] must raise EntityDoesNotExistException and a repeat DeleteDirectory must also raise EntityDoesNotExistException.
