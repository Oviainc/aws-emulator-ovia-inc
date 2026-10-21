Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: aa53e03837fde54d44dc5b021f911ec63548de2f

# Bootstrap the Signer Data (signerdataplane) emulator

_Filed: 2026-10-21T22:22:48Z_
_Operation: service-com-amazonaws-signerdata-signerdataplane | Class: stub_

> _Note: this issue was identified against the code base at commit `aa53e03837fde54d44dc5b021f911ec63548de2f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate `services/service-com-amazonaws-signerdata-signerdataplane` is a fresh scaffold: its only handler, `src/ops/get_revocation_status.rs`, still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented("GetRevocationStatus"))` (HTTP 501). The service binds exactly ONE operation, so the happy-path slice is that single operation plus the revocation state it must read.

## Happy path to implement (in order)

1. **Seeded revocation state** (test/fixture seam, not an AWS operation) — the data plane is read-only (`GetRevocationStatus` is `@readonly`); the revocations it reports are produced by the Signer control plane (`RevokeSignature` / `RevokeSigningProfile`). The emulator must therefore hold a real, inspectable revocation registry in `crate::state::service_state` that a test (or a sibling signer control-plane crate, when it exists) can populate. No canned responses.
2. **GetRevocationStatus** — `GET /revocations` (restJson1, sigv4 signing name `signer`, endpoint prefix `data-signer`).
   - Input (all `@required`, all `@httpQuery`): `signatureTimestamp` (timestamp), `platformId` (string, pattern `^[a-zA-Z0-9_-]+$`), `profileVersionArn` (string, len 20..2048), `jobArn` (string, len 20..2048), `certificateHashes` (list<string>).
   - Output: `revokedEntities` (list<string>) — the identifiers of the entities that are revoked for this signature, EMPTY (not absent-with-garbage) when nothing matches.
   - Errors: `ValidationException` (400), `AccessDeniedException` (403), `TooManyRequestsException` (429), `InternalServiceErrorException` (500).

## Required behaviour (no canned or echoed responses)

- `revokedEntities` MUST be computed by looking each of `profileVersionArn`, `jobArn` and every entry of `certificateHashes` up in the persisted registry, and returning only those found — never by echoing the input, never a hardcoded empty list, never a hardcoded non-empty list.
- Revocation is effective-time scoped, per well-established AWS Signer semantics: a revoked profile version / signing job invalidates only signatures whose `signatureTimestamp` is at or after the revocation's effective time. A revocation entry whose effective time is LATER than the requested `signatureTimestamp` MUST NOT appear in `revokedEntities`.
- Repeated calls with the same input must return the same answer (the read must not mutate state); adding a revocation to the registry must become visible to the next call, and removing it must become invisible.
- Multiple revoked entities (e.g. both the job ARN and one certificate hash) must all appear in one response.
- Input constraints from the model (`platformId` pattern, 20..2048 ARN lengths, required query params) are refused BEFORE `handle` with a 400 `ValidationException`; a request missing a required query parameter must be a 400, not a 501 and not a 200.

## State Model

In `crate::state` (one `ServiceState` behind the existing lock):

- `revoked_profile_versions: HashMap<String /* profileVersionArn */, RevocationRecord>`
- `revoked_jobs: HashMap<String /* jobArn */, RevocationRecord>`
- `revoked_certificates: HashMap<String /* certificate hash */, RevocationRecord>`
- `RevocationRecord { entity_id: String /* value echoed into revokedEntities */, effective_time: SystemTime /* revocation effective time */, reason: Option<String>, platform_id: Option<String> /* when set, only matches requests with this platformId */ }`

Lookup algorithm for `GetRevocationStatus`: start with an empty `Vec<String>`; for the input `profileVersionArn`, `jobArn`, and each `certificateHashes` entry, fetch the record from the matching map, skip it when `record.effective_time > input.signatureTimestamp` or when `record.platform_id` is set and differs from `input.platformId`, otherwise push `record.entity_id`. Return `revokedEntities` = that vector (possibly empty).

Nothing else is in scope for this first slice — this service models no other operations.

## Evidence

`services/service-com-amazonaws-signerdata-signerdataplane/OPERATIONS.md` lists exactly one operation, `GetRevocationStatus` — `GET /revocations`, protocol restJson1, sigv4 name `signer`, endpoint prefix `data-signer` — with required query inputs `signatureTimestamp`, `platformId` (pattern `^[a-zA-Z0-9_-]+$`), `profileVersionArn` (len 20..2048), `jobArn` (len 20..2048), `certificateHashes` (list<string>), output `revokedEntities` (list<string>), and errors AccessDeniedException/403, InternalServiceErrorException/500, TooManyRequestsException/429, ValidationException/400. The model doc embedded in the handler states the operation "Retrieves the revocation status for a signed artifact by checking if the signing profile, job, or certificate has been revoked" and that `revokedEntities` is "Empty if no revocations found." The handler `src/ops/get_revocation_status.rs:152-170` binds `(&input, ctx, &self.engine)` to `_` and returns `Scaffolded::not_implemented`, i.e. HTTP 501 with zero state. No tests exist for this crate. The effective-time scoping is grounded in AWS Signer's documented revocation model (RevokeSignature/RevokeSigningProfile take an `effectiveTime`; signatures signed at or after that time are treated as revoked).

## Steps to Reproduce

1. Start the emulator and seed the revocation registry with a revoked signing job (jobArn `arn:aws:signer:us-east-1:111122223333:/signing-jobs/job-1`, effective time T0) and a revoked certificate hash (`abc123...`, effective time T0).
2. Call `GET /revocations?signatureTimestamp=<T0+1h>&platformId=AWSLambda-SHA384-ECDSA&profileVersionArn=arn:aws:signer:us-east-1:111122223333:/signing-profiles/p/ver1&jobArn=arn:aws:signer:us-east-1:111122223333:/signing-jobs/job-1&certificateHashes=abc123...&certificateHashes=def456...` — expect HTTP 200 and `revokedEntities` containing the job ARN and `abc123...` but NOT `def456...` and NOT the profile version ARN. Today: HTTP 501 NotImplemented.
3. Repeat the identical call — expect the same `revokedEntities` (the read must not mutate state).
4. Call the same request with `signatureTimestamp=<T0-1h>` (signature predates the revocation effective time) — expect HTTP 200 with `revokedEntities` empty.
5. Remove the certificate hash revocation from the registry and call again — expect `revokedEntities` to contain only the job ARN, proving the answer is computed from persisted state rather than canned.
6. Call `GET /revocations?platformId=AWSLambda-SHA384-ECDSA` (missing required query params) — expect HTTP 400 `ValidationException`, not 501 and not 200.
7. Call with `platformId=bad id!` (violates `^[a-zA-Z0-9_-]+$`) and with a 5-character `jobArn` (violates len 20..2048) — expect HTTP 400 `ValidationException` for each.
