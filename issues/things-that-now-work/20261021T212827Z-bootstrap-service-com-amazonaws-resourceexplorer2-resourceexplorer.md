Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the Resource Explorer 2 emulator

_Filed: 2026-10-21T21:28:27Z_
_Operation: service-com-amazonaws-resourceexplorer2-resourceexplorer | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 28 wired handlers under `services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and the crate holds no state. Implement a FIRST, SMALL, stateful slice: the index/view lifecycle that every Resource Explorer client starts with (restJson1, signing name `resource-explorer-2`).

### Happy path to implement (in order)

1. **CreateIndex** — `POST /CreateIndex`. Input: `ClientToken` (@idempotencyToken), `Tags` (map<string,string>). Must create at most ONE index per account+region, store it, and return `Arn` (e.g. `arn:aws:resource-explorer-2:<region>:<account>:index/<uuid>`), `State` = `ACTIVE` (the emulator may settle directly to ACTIVE rather than CREATING), `CreatedAt`. A second CreateIndex for a region that already has an index must return `ConflictException` (HTTP 409); repeating the same `ClientToken` must return the SAME index rather than creating a second one. `Type` defaults to `LOCAL`.
2. **CreateView** — `POST /CreateView`. Input: `ViewName` (required, pattern `^[a-zA-Z0-9\-]{1,64}$`), `Filters` (SearchFilter{FilterString}), `IncludedProperties` (list<IncludedProperty{Name}>), `Scope`, `Tags`, `ClientToken`. Requires an existing index in the region, else fail (not 501). Must persist a `View` and return it fully populated: `ViewArn` (`arn:aws:resource-explorer-2:<region>:<account>:view/<ViewName>/<uuid>`), `ViewName`, `Owner` (account id), `LastUpdatedAt`, `Scope`, `IncludedProperties`, `Filters` — echoing the request is NOT acceptable unless the same values are read back out of the store by GetView. Duplicate `ViewName` in the region → `ConflictException` (409).
3. **ListViews** — `POST /ListViews`. Input: `MaxResults`, `NextToken`. Output `Views` (list of view ARNs) computed from the store, honouring `MaxResults` and round-tripping `NextToken` (@paginated inputToken=NextToken/outputToken=NextToken/items=Views). The ARN returned by step 2 must appear here.
4. **GetView** — `POST /GetView`. Input: `ViewArn` (required). Output: `View` (the persisted structure, identical to what CreateView returned) plus `Tags` from the stored tag map. An unknown or deleted ARN → `ResourceNotFoundException` (HTTP 404).
5. **Search** — `POST /Search`. Input: `QueryString` (required, length 0..1280), `ViewArn`, `MaxResults`, `NextToken`. Resolve the view (explicit `ViewArn`, else the default view set by AssociateDefaultView, else 404 `ResourceNotFoundException`), match `QueryString` against a seeded/registered resource inventory, apply the view's `Filters.FilterString`, and return `Resources` (list<Resource>), `ViewArn` of the view actually used, and `Count` (ResourceCount{TotalResources, Complete}) consistent with the returned list. `MaxResults`/`NextToken` must paginate. A hardcoded empty-or-canned `Resources` with a `Count` that does not match the list is a failure.
6. **DeleteView** — `POST /DeleteView`. Input: `ViewArn` (required). Remove the view from the store and return `ViewArn`. Afterwards GetView on that ARN must be `ResourceNotFoundException` (404) and the ARN must be gone from ListViews. Deleting an unknown ARN → 404.

Optional same-slice extras once the above pass (cheap, same state): **ListIndexes** (`Indexes` list of Index{Region, Arn, Type}, so the index from step 1 is observable), **AssociateDefaultView** (store the default view ARN used by Search), **DeleteIndex** (mark state `DELETED`/remove and reject view creation afterwards).

### NEVER STUBS
Every operation above must compute its response from persisted, cross-request state held behind `crate::state::service_state(...)`. No hardcoded ARNs, no echoed inputs that are never stored, no random IDs that are not written to the store. A create followed by a read must return exactly what was created.

## State Model

```
ResourceExplorerState {
    // one index per (account, region); the emulator may hold a single region
    indexes: HashMap<String /*region*/, Index {
        arn: String,              // arn:aws:resource-explorer-2:<region>:<acct>:index/<uuid>
        region: String,
        index_type: IndexType,    // LOCAL | AGGREGATOR
        state: IndexState,        // ACTIVE | CREATING | DELETING | DELETED | UPDATING
        created_at: Timestamp,
        last_updated_at: Timestamp,
        replicating_from: Vec<String>,
        replicating_to: Vec<String>,
        tags: HashMap<String, String>,
        client_token: Option<String>,   // for @idempotencyToken replay
    }>,

    views: HashMap<String /*view ARN*/, ViewEntry {
        view_arn: String,         // arn:...:view/<name>/<uuid>
        view_name: String,        // unique per region
        owner: String,            // account id
        scope: String,            // defaults to account ARN
        filters: Option<String>,  // SearchFilter.FilterString
        included_properties: Vec<String>,  // IncludedProperty.Name, e.g. "tags"
        last_updated_at: Timestamp,
        tags: HashMap<String, String>,
        client_token: Option<String>,
    }>,
    views_by_name: HashMap<String /*view name*/, String /*view ARN*/>, // conflict detection

    default_view_arn: Option<String>,   // AssociateDefaultView / GetDefaultView / Search fallback

    // inventory Search/ListResources read from (seeded or registered by tests)
    resources: Vec<ResourceEntry {
        arn: String,
        owning_account_id: String,
        region: String,
        service: String,          // e.g. "ec2"
        resource_type: String,    // e.g. "ec2:instance"
        last_reported_at: Timestamp,
        properties: Vec<(String /*name*/, Json /*data*/)>,  // e.g. tags
    }>,

    // opaque pagination cursors for ListViews / Search / ListIndexes
    pagination: HashMap<String /*NextToken*/, usize /*offset*/>,
}
```

All other operations (BatchGetView, UpdateView, UpdateIndexType, ListResources, Tag/UntagResource, ListTagsForResource, the managed/service-view and setup-task families, …) stay 501 and are filed as individual gap issues later.

## Evidence

OPERATIONS.md at the crate root (`services/service-com-amazonaws-resourceexplorer2-resourceexplorer/OPERATIONS.md`) lists all 32 modeled operations with routes, inputs, outputs and errors: CreateIndex `POST /CreateIndex` → `Arn`/`State`/`CreatedAt` with ConflictException(409); CreateView `POST /CreateView` with required `ViewName` (pattern ^[a-zA-Z0-9\-]{1,64}$) → `View`; ListViews `POST /ListViews` @paginated(items=Views); GetView `POST /GetView` required `ViewArn` → `View`+`Tags` with ResourceNotFoundException(404); Search `POST /Search` required `QueryString` (len 0..1280) → `Resources`/`NextToken`/`ViewArn`/`Count` @paginated(items=Resources); DeleteView `POST /DeleteView` required `ViewArn` → `ViewArn`. Shape detail from smithy_query: View{ViewArn, ViewName, Owner, LastUpdatedAt, Scope, IncludedProperties, Filters}, Index{Region, Arn, Type}, SearchFilter{FilterString required, len 0..2048}, IncludedProperty{Name required}. Stub proof: `src/ops/create_view.rs` lines 4 and 203 — the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateView"))`; all 28 wired op files are identical scaffolds, and `src/ops/SKIPPED_STUBS.md` shows GetIndex/GetDefaultView/GetServiceIndex/GetAccountLevelServiceConfiguration/DisassociateDefaultView are not even wired (Unit input), which is why the happy path above avoids them. No tests assert any of this behaviour.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with `--endpoint-url`, service `resource-explorer-2`.
2. `aws resource-explorer-2 create-index` → expect 200 with an index `Arn`, `State` ACTIVE, `CreatedAt`; today: HTTP 501 NotImplemented.
3. Repeat `create-index` with the same client token → expect the SAME Arn back (idempotent); with a different token → expect ConflictException (409).
4. `aws resource-explorer-2 create-view --view-name my-view --filters FilterString=service:ec2` → expect 200 with View.ViewArn, ViewName=my-view, Owner, LastUpdatedAt, Filters echoed from stored state.
5. `aws resource-explorer-2 list-views` → expect the new view ARN in `Views`; pass `--max-results 1` with multiple views and follow `NextToken` to page through all of them.
6. `aws resource-explorer-2 get-view --view-arn <arn from step 4>` → expect the identical View structure plus `Tags`.
7. `aws resource-explorer-2 search --query-string "service:ec2" --view-arn <arn>` → expect `Resources` from the stored inventory, `ViewArn` of the view used, and `Count.TotalResources` equal to the number of matching resources.
8. `aws resource-explorer-2 delete-view --view-arn <arn>` → expect 200 with `ViewArn`; then `get-view` on it → expect ResourceNotFoundException (404) and the ARN absent from `list-views`.
9. `aws resource-explorer-2 get-view --view-arn arn:aws:resource-explorer-2:us-east-1:111122223333:view/nope/00000000-0000-0000-0000-000000000000` → expect ResourceNotFoundException (404), not 501.
