Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 5c8ac701ad3e8d191ad2ab71e1d77d8b6db767d3

# An implemented CreateCliToken answers a server-side fault with the scaffold's 501 "not implemented", unlike its sibling CreateWebLoginToken

_Filed: 2026-10-29T06:21:38Z_
_Operation: CreateCliToken | Class: error-response_

> _Note: this issue was identified against the code base at commit `5c8ac701ad3e8d191ad2ab71e1d77d8b6db767d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateCliToken` is fully implemented (it resolves the stored environment, reads the host off the stored `WebserverUrl` and mints a token), yet two of its failure branches return `crate::scaffolded::Scaffolded::not_implemented("CreateCliToken")`, i.e. HTTP 501 with the body message "operation CreateCliToken is scaffolded but not implemented in this emulator":

1. the store being unreachable (`service_state` -> `None`), and
2. the credential store minting no secret (`mint_airflow_token` -> `None`).

Both are server-side faults of an operation that IS written, so the answer makes a claim the crate's own error channel explicitly reserves for an UNWRITTEN handler, and which `scaffolded.rs` says "is counted as one" (i.e. counted as an unimplemented operation). The sibling handler on the same `env.` endpoint, `CreateWebLoginToken`, answers the very same two conditions with a modelled `InternalServerException` (HTTP 500) — so one of the two token operations reports "the emulator is broken" and the other reports "nobody has written this yet" for one identical condition. A client (or a human triaging a fixture) that calls `CreateCliToken` and receives 501 `NotImplemented` concludes the operation is a scaffold stub, which is false.

The defect is the claim the refusal makes, not the absence of a modelled 500: the crate already carries non-modelled wire answers for exactly this situation (`Scaffolded::validation` and `Scaffolded::NotFound`, used by the tagging operations), so the 501 is a choice, not a constraint.

## Evidence

Both repo sides, quoted:

services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/create_cli_token.rs:177-181 — an implemented handler answering a server fault with the scaffold signal:
```
let Some(cli_token) = crate::state::mint_airflow_token(ctx.credential_store()) else {
    // No secret was minted, and this operation will not invent one; it models no
    // server fault, so the honest answer is "this emulator did not serve it".
    return Err(crate::scaffolded::Scaffolded::not_implemented("CreateCliToken"));
};
```
and create_cli_token.rs:154-156 for the store:
```
let Some(state) = crate::state::service_state(&*self.engine) else {
    return Err(crate::scaffolded::Scaffolded::not_implemented("CreateCliToken"));
};
```

The sibling, services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/create_web_login_token.rs:194-201, answers the identical mint failure as a modelled server fault:
```
let web_token = crate::state::mint_airflow_token(ctx.credential_store()).ok_or_else(|| {
    CreateWebLoginTokenErrorModeled::InternalServerException(InternalServerException {
        message: Some(
            "the emulator minted no Apache Airflow web login token for this request"
                .to_string(),
        ),
    })
})?;
```
and create_web_login_token.rs:165-169 does the same for the missing store.

The crate's own rule for what that 501 must mean — services/service-com-amazonaws-mwaa-amazonmwaa/src/scaffolded.rs:90-99:
```
/// The wording matters: it must read as "not written yet", never as "the emulator
/// is broken", because that distinction is what a human triaging a failing fixture
/// needs first.
pub fn new(operation: &str) -> Self {
    Self { message: format!("operation {operation} is scaffolded but not implemented in this emulator") }
}
```
and scaffolded.rs:274-277:
```
/// Answer it INSTEAD of `not_implemented` — a 501 says "nobody has written this yet",
/// which is a different claim and is counted as one.
```
scaffolded.rs:299-301 shows the crate already answers a wire code its model does not declare (`Scaffolded::not_found`, used by list_tags_for_resource.rs:161, tag_resource.rs:183, untag_resource.rs:181), so a non-modelled answer is available here too.

Test coverage: tests/all/rejection_contract.rs:30-33 pins only the happy path ("`CreateWebLoginToken` and `CreateCliToken` mint a token for an environment that EXISTS") and tests/all/env_endpoint_payload.rs covers the 404/400 envelopes; no test pins either of the two 501 branches, so nothing depends on the current answer.

## Steps to Reproduce

1. Read services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/create_cli_token.rs:154-156 and :177-181 — an implemented handler returns `Scaffolded::not_implemented("CreateCliToken")` (HTTP 501, message "operation CreateCliToken is scaffolded but not implemented in this emulator") for a missing store and for a failed token mint.
2. Read services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/create_web_login_token.rs:165-169 and :194-201 — the sibling operation on the same `env.` endpoint answers the same two conditions with the modelled `InternalServerException` (HTTP 500).
3. Read services/service-com-amazonaws-mwaa-amazonmwaa/src/scaffolded.rs:90-99 and :274-277 — the crate states the 501 message "must read as 'not written yet', never as 'the emulator is broken'" and that answering it "is counted as one" unimplemented operation.
4. Confirm CreateCliToken is not a scaffold: `grep -n 'SCAFFOLDED, NOT IMPLEMENTED' services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/*.rs` matches only publish_metrics.rs.
