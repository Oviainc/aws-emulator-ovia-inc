Issue-Source: code-review-followup
Issue-Severity: LOW

# The awsJson fault-status rule is written out once per service crate instead of once in `services/common`

## Operation
(none — a shared-code duplication, not a wire defect)

## Description
Three service crates each hold their own copy of the SAME rule: on the `awsJson` protocols a
service answers a modelled error at its FAULT rather than at the exact code the shape's
`@httpError` trait names, so every modelled client error renders HTTP 400 (the body's
`__type` is what distinguishes one from another) and a server error keeps its 5xx.

The copies are byte-identical in body and near-identical in doc:

- `services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/scaffolded.rs` — `wire_http_status`
- `services/service-com-amazonaws-healthlake-healthlake/src/scaffolded.rs` — `wire_http_status`
- `services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/scaffolded.rs` — `wire_http_status`

Each is read by that crate's `impl OperationError for Scaffolded<E>::http_status`, so the
rule decides the HTTP status of EVERY modelled error those services answer. Three copies
are three places it can drift: a fourth service hitting the same recorded divergence will
add a fourth, and a correction made in one leaves the others wrong.

`services/common` is where this emulator already states a rule more than one service needs
— `rpc_target` is the precedent, holding the ONE reading of an `awsJson` `X-Amz-Target` for
every service whose published target prefix carries dots. The fault-status rule belongs
beside it, as one public function the crates read.

The evidence behind the rule is per-FAULT and already recorded in those crates' docs: the
wire recording of Amazon EVS answers `ResourceNotFoundException` — `@httpError(404)` in its
published model — with HTTP 400. The consolidation should carry that statement, and its test
should assert the mapping over a table of the `@httpError` values the models actually
declare (402, 404, 409, 429 -> 400; 500 -> 500) rather than one value. Whether a GIVEN
service answers at the fault or at the modelled status stays that service's own decision,
grounded in its own recording: CodeStar connections, for instance, answers its modelled
statuses (404 for `ResourceNotFoundException`, 429 for `LimitExceededException`), so it is
not one of the callers.

## Why this matters
A service's error STATUS is part of its wire contract: an SDK's retry classifier branches
on it, so a service that renders a client error at one status where the real one renders
another makes a caller retry (or give up) differently than it would against AWS. That
decision is currently made in three independent copies of one function.

## Evidence
Reported by the merge gate's `code_reviewer` on the CodeStar connections target-routing fix
(`issues/things-that-now-work/20261023T122922Z-codestar-connections-implemented-connection-operations-answer-501-notimplemented-instead-of.md`),
as a WARNING repeated across several review rounds: a hand-written fault-status mapping is
an extra copy of an identical mapping that already exists in the tree, and "the rule can be
stated once there (`service_common::` alongside `rpc_target`) and read by all the crates."

## Steps to Reproduce
1. `grep -rn "fn wire_http_status" services/*/src/scaffolded.rs` — three hits, same body
   (`if modeled_status >= 500 { modeled_status } else { 400 }`).
2. Read each one's doc comment: the same rule and the same recorded-wire justification,
   restated three times.
3. Note that `services/common/src/rpc_target.rs` holds exactly this kind of rule once, for
   the same set of services.

## Fix
Add the function to `services/common` (its own module, declared in `services/common/src/lib.rs`
and listed in that file's "Code more than one emulated service needs" header), with the
rule, the recorded-wire evidence and a table-driven unit test over the `@httpError` values
the models declare. Then have each of the three crates call it and delete its copy. Run the
workspace tests, since `services/common` is a path dependency of every service crate.
