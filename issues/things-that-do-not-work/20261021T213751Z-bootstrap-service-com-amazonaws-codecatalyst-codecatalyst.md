Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the CodeCatalyst emulator

_Filed: 2026-10-21T21:37:51Z_
_Operation: service-com-amazonaws-codecatalyst-codecatalyst | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 38 operations in `services/service-com-amazonaws-codecatalyst-codecatalyst/src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501. `src/state.rs` holds `ComAmazonawsCodecatalystCodecatalystState { /* TODO: add service-specific state fields */ }` — the service stores nothing. There is no working emulator yet.

Implement a FIRST, SMALL, STATEFUL slice: the space -> project -> source-repository core loop of CodeCatalyst (restJson1, endpoint prefix `codecatalyst`). Note that the API models NO CreateSpace operation, so the space is an ambient/seeded resource the emulator must provide (a default space, e.g. `my-space`, created at state init or lazily on first reference).

Happy-path operations, in order:

1. **ListSpaces** — `POST /v1/spaces`. Input: `nextToken`. Output: `items` (list<SpaceSummary>: `name`, `regionName`, `displayName`, `description`), `nextToken`. Must list the seeded spaces from state, not a canned literal.
2. **CreateProject** — `PUT /v1/spaces/{spaceName}/projects` -> 201. Input: `spaceName` (@httpLabel, required), `displayName` (required, len 3..63), `description`. Output: `spaceName`, `name` (required), `displayName`, `description`. Must insert a Project into the space's project map, derive and persist `name` from `displayName`, and reject a `spaceName` that does not exist.
3. **GetProject** — `GET /v1/spaces/{spaceName}/projects/{name}` -> 200. Must return exactly the stored project (`spaceName`, `name`, `displayName`, `description`) and fail for an unknown project/space — never echo the request path back as a synthesized project.
4. **ListProjects** — `POST /v1/spaces/{spaceName}/projects` -> 200. Input: `nextToken`, `maxResults`, `filters`. Output: `items` (list<ProjectSummary>), `nextToken`. Must enumerate the projects actually created in that space and honour `maxResults`/`nextToken` (emit `nextToken` only when a page was truncated; a returned token must resume where the previous page ended).
5. **CreateSourceRepository** — `PUT /v1/spaces/{spaceName}/projects/{projectName}/sourceRepositories/{name}` -> 201. Input: `spaceName`, `projectName`, `name` (len 1..100, pattern `^(?!.*[.]git$)[\w\-.]*$`), `description`. Output: `spaceName`, `projectName`, `name`, `description`. Must store the repository under its project with `createdTime`/`lastUpdatedTime` so the (later) GetSourceRepository/ListSourceRepositories can report it, and must 4xx when the parent project does not exist.
6. **DeleteProject** — `DELETE /v1/spaces/{spaceName}/projects/{name}` -> 200. Output: `spaceName` (required), `name` (required), `displayName`. Must actually remove the project (and its source repositories) from state so a following GetProject fails and ListProjects no longer returns it.

Hard requirements:
- NEVER STUBS: every one of the six must compute its response from persisted state. Hardcoded bodies, echoing the path labels back, or generating an id without storing anything are failures.
- Create -> read consistency: whatever CreateProject/CreateSourceRepository stored is exactly what the read/list operations return.
- Spec-grounded validation: enforce the `@httpLabel`/body member length and pattern constraints from OPERATIONS.md (space/project names len 3..63 pattern `^[a-zA-Z0-9]+(?:[-_\.][a-zA-Z0-9]+)*$`, description len 0..200, repository name pattern above) and reject references to non-existent parents with a modelled 4xx rather than a 200 or a 501.
- Done = a realistic integration script (below) passes end-to-end with correct bodies, not merely 2xx.

## State Model

In `ComAmazonawsCodecatalystCodecatalystState` (reached via `crate::state::service_state(&*self.engine)`):

- `spaces: BTreeMap<String /*spaceName*/, Space>` — seeded with one default space at init.
- `Space { name: String, region_name: String /* e.g. "us-west-2" */, display_name: Option<String>, description: Option<String>, projects: BTreeMap<String /*project name*/, Project> }`
- `Project { space_name: String, name: String, display_name: Option<String>, description: Option<String>, source_repositories: BTreeMap<String /*repo name*/, SourceRepository> }`
- `SourceRepository { space_name: String, project_name: String, name: String, description: Option<String>, created_time: SystemTime, last_updated_time: SystemTime, branches: BTreeMap<String, Branch> /* empty in this slice; the branch ops stay stubbed */ }`
- Pagination: `nextToken` is an opaque cursor over the sorted key order of the map being listed (e.g. the last returned key, base64-encoded), used by ListSpaces/ListProjects.

The other 32 operations (dev environments, workflows, workflow runs, access tokens, event logs, subscription, user/session, source-repository branches and clone URLs, GetSpace/UpdateSpace/DeleteSpace, UpdateProject, GetSourceRepository/ListSourceRepositories/DeleteSourceRepository) intentionally stay 501 in this slice and are tracked as individual gap issues later.

## Evidence

`services/service-com-amazonaws-codecatalyst-codecatalyst/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-codecatalyst-codecatalyst.json`) lists all 38 operations with routes and members: ListSpaces `POST /v1/spaces`, CreateProject `PUT /v1/spaces/{spaceName}/projects -> 201`, GetProject `GET /v1/spaces/{spaceName}/projects/{name}`, ListProjects `POST /v1/spaces/{spaceName}/projects` (@paginated nextToken/items), CreateSourceRepository `PUT /v1/spaces/{spaceName}/projects/{projectName}/sourceRepositories/{name} -> 201`, DeleteProject `DELETE /v1/spaces/{spaceName}/projects/{name}`; the model declares no CreateSpace. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches every operation file (create_project.rs, get_project.rs, list_projects.rs, list_spaces.rs, create_source_repository.rs, delete_project.rs, ...), each returning `Scaffolded::not_implemented`. `src/state.rs` has no state fields. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI CodeCatalyst client at it.
2. ListSpaces -> expect 200 with at least the seeded space (e.g. name `my-space`, a regionName); today: 501 NotImplemented.
3. CreateProject(spaceName="my-space", displayName="my-project", description="demo") -> expect 201 with name/displayName/description echoing the stored project.
4. GetProject(spaceName="my-space", name="my-project") -> expect 200 returning the description "demo" that was persisted by step 3.
5. ListProjects(spaceName="my-space", maxResults=1) -> expect items to contain my-project; with more projects than maxResults expect a nextToken that resumes correctly.
6. CreateSourceRepository(spaceName="my-space", projectName="my-project", name="my-repo", description="src") -> expect 201 and the repo persisted under the project.
7. CreateSourceRepository with projectName="no-such-project" -> expect a modelled 4xx, not 200/501.
8. DeleteProject(spaceName="my-space", name="my-project") -> expect 200 with spaceName+name; then GetProject for it fails and ListProjects no longer lists it.
