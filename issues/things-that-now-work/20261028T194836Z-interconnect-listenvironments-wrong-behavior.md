Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a67556f919df77bd586efc9ebe2e6b88c4f9f9e7

# ListEnvironments discards the called region and answers the same catalog on every regional endpoint

_Filed: 2026-10-28T19:48:36Z_
_Operation: ListEnvironments | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a67556f919df77bd586efc9ebe2e6b88c4f9f9e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model — and the handler's own generated contract header — say that ListEnvironments lists the environments "that can produce connections that will land in the called AWS region", so the answer is a function of the region the request was signed for. The handler opens by throwing that information away (`let _ = ctx;`) and returns the whole seeded catalog, unfiltered, on every endpoint. Every one of this crate's other eleven state-reading handlers resolves `crate::state::request_region(ctx)` first and scopes its answer by it; this one is the single handler that discards `ctx`.

The consequence is observable: a caller signed for ap-southeast-2 is offered `env-csp-azure-us-east-1`, `env-csp-googlecloud-us-west-2` and `env-csp-oraclecloud-eu-west-1`, and `CreateConnection` signed for that same region then accepts a placement on any of them, so the connection ARN the create mints carries ap-southeast-2 for an environment the listing presented as landing somewhere else. Two different regional endpoints of the emulator can never be told apart by their environment catalogs, which is exactly the distinction the quoted sentence exists to express.

## Evidence

The rule, from the model and quoted verbatim in the handler's own header — services/service-com-amazonaws-interconnect-interconnect/src/ops/list_environments.rs:43-44:
```
//! Lists all of the environments that can produce connections that will land in the called
//! AWS region.
```
(identical to `smithy_query(query="doc",shape="ListEnvironments")`: "Lists all of the environments that can produce connections that will land in the called AWS region.")

The code under that header, which discards the request's region — list_environments.rs:134-139:
```
    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
```
and the only filtering it applies, which is the two modelled filters and nothing else — list_environments.rs:156-175:
```
        let matching: Vec<(String, Environment)> = {
            let store = state.lock();
            store
                .environments
                .iter()
                .filter(|(_, environment)| {
                    input
                        .provider
                        .as_ref()
```

The sibling rule this skips, which every other reader of this store applies — src/ops/list_connections.rs:144:
```
        let region = crate::state::request_region(ctx);
```
with the helper it calls documented at src/state.rs:494-499:
```
/// The region this request was signed for, which is the region every ARN the
/// request mints carries. Falls back to [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
```
and the crate's own statement that this service's answers are regional — src/state.rs:12-15:
```
//! * **Connections** — caller-created. `CreateConnection` allocates one and every
//!   other connection operation reads or mutates that stored record. This service's
//!   endpoints are regional, so each record belongs to the Region its create was
//!   signed for and is resolved only for requests signed for that Region.
```

Test coverage: tests/all/region_scoped_connections.rs pins the regional scope for CONNECTIONS only; no test calls ListEnvironments through two different signing regions, so nothing pins the catalog's region-independence. (Distinct from the open trace issue 20261023T095130Z, whose Cause 1 is about WHICH environments the catalog holds, not about whether the called region selects among them.)

## Steps to Reproduce

1. Boot the emulator.
2. Call ListEnvironments signed for us-east-1 and record the returned environmentIds.
3. Call ListEnvironments signed for ap-southeast-2 (as tests/all/emulator.rs `ic_ok_in` does for other operations) and record the returned environmentIds.
4. Observe both calls return the identical five seeded environments, although src/ops/list_environments.rs:43-44 states the listing is of environments "that can produce connections that will land in the called AWS region".
5. Confirm the cause: list_environments.rs:139 is `let _ = ctx;`, so `crate::state::request_region(ctx)` (src/state.rs:496) is never called here, unlike in src/ops/list_connections.rs:144.
6. Follow through: CreateConnection signed for ap-southeast-2 on env-csp-azure-us-east-1 succeeds, and GetConnection returns an ARN whose region field is ap-southeast-2 for that environment.
