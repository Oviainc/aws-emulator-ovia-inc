Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Enforce the us-east-1-only restriction on the Lightsail domain operations

_Filed: 2026-10-24T01:05:05Z_
_Operation: CreateDomain | Class: validation_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Lightsail's domain (DNS zone) API is served ONLY by the N. Virginia (us-east-1) endpoint. A CreateDomain call made against any other Region's endpoint is refused by the real service with the modelled InvalidInputException (HTTP 400) whose message is exactly "Domain and distribution APIs are only available in the N. Virginia (us-east-1) Amazon Web Services Region. Please set your Amazon Web Services Region configuration to us-east-1 to create, view, or edit these resources."

Expected: a CreateDomain request signed for, say, us-west-2 or eu-west-1 answers HTTP 400 InvalidInputException with that message and creates no zone.

Actual: the handler deliberately discards the request's Region (`let _ = ctx;`) and stores the zone with `StoredLocation::domain()` (hard-coded region `us-east-1`), so a create from ANY Region succeeds with a 200 and an operation whose location claims us-east-1. The same hole exists on the other implemented domain operations, which also ignore the request Region and should be refused outside us-east-1: CreateDomainEntry, GetDomain, UpdateDomainEntry and DeleteDomainEntry — a zone created through a us-west-2 endpoint is readable and editable through it too.

## Evidence

Spec: the Smithy model's `InvalidInputException` documentation (shared by CreateDomain/GetDomain/CreateDomainEntry/UpdateDomainEntry/DeleteDomainEntry) states the us-east-1-only rule verbatim; the emulator's own `src/state.rs` quotes that same sentence at `DOMAIN_REGION_NAME` ("Domain and distribution APIs are only available in the N. Virginia (us-east-1) Amazon Web Services Region") yet nothing checks the REQUEST's Region. Handler: `src/ops/create_domain.rs` (handle body: `let _ = ctx;` then `StoredLocation::domain()`), plus `src/ops/create_domain_entry.rs`, `src/ops/get_domain.rs`, `src/ops/update_domain_entry.rs`, `src/ops/delete_domain_entry.rs`, all of which discard `ctx`. Tests: `tests/all/domain_zone.rs` only asserts the reported location is the domain region; no test exercises a non-us-east-1 endpoint, so the refusal is uncovered.

## Steps to Reproduce

1. Send CreateDomain {"domainName": "example.com"} to the emulator with a SigV4 credential scope for us-west-2 (or any Region other than us-east-1)
2. Observe HTTP 200 with an operation whose location.regionName is us-east-1; real Lightsail answers HTTP 400 InvalidInputException naming the us-east-1-only restriction
3. Follow with GetDomain {"domainName": "example.com"} and CreateDomainEntry against the same non-us-east-1 endpoint and observe both succeed as well
