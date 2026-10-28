Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 15aaa5471fc3f7278352228b97d2216e65bea584

# src/service/response_media_type.rs re-states the response-media-type rule service-common already provides

_Filed: 2026-10-28T14:46:36Z_
_Operation: GetDashboard | Class: maintainability_

> _Note: this issue was identified against the code base at commit `15aaa5471fc3f7278352228b97d2216e65bea584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate carries its own copy of the "re-announce the response media type" mechanism — `with_response_media_type`, `routes` and the `ResponseMediaTypeOperation` wrapper that serves the framework's refusal tail — in `src/service/response_media_type.rs`. The identical mechanism lives in `services/common/src/response_media_type.rs` as `ResponseMediaType::announce`, `routes` and `ResponseMediaTypeOperation`, and `service-common` is ALREADY a dependency of this crate (Cargo.toml:35, used today for `service_common::validation::unanchored` and `service_common::validation_message::aws_validation_refusal`). The shared module names this crate by name as the copy still to be migrated, so the two copies are known to be the same rule and can drift apart — a change to how a response is re-announced has to be made twice, in two crates, to stay consistent.

The replacement is value-for-value: `ResponseMediaType::new(AWS_JSON_1_0, AWS_JSON_1_1)` holds the same two constants this file spells as `PROTOCOL_MEDIA_TYPE`/`RESPONSE_MEDIA_TYPE`, `announce` performs the same whole-header comparison and insert, the shared `ResponseMediaTypeOperation` resolves the routing key and delegates to the modelled operation exactly as the local one does (and returns the same `None` from `auth_schemes`), and the shared `routes` asks the same `definition.protocol().routing_key(req)` / `routing_table().get(key)` question through `&dyn ErasedServiceDefinition`. Every status, header and body byte therefore stays as it is, which is what the existing suites pin.

## Evidence

The shared module says this crate's copy is the duplicate — services/common/src/response_media_type.rs:59-73:
```
//! rather than in that crate because it is not that service's rule alone: the sibling
//! `service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice` — the same AWS billing
//! front door, the same `1.1` framing over an `awsJson1_0` model — states it inline today
//! (`src/service/response_media_type.rs`), ...
//! STILL TO MIGRATE: those three onto this module — `bcmdashboards` including its own copy
//! of [`ResponseMediaTypeOperation`].
```

The two copies of the one replacement:
- services/common/src/response_media_type.rs:128-133 — `pub fn announce(&self, mut resp: RtResponse) -> RtResponse { if resp.headers().get(CONTENT_TYPE) == Some(self.implied) { resp.headers_mut().insert(CONTENT_TYPE, self.announced); } resp }`
- services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/service/response_media_type.rs:77-82 — `pub fn with_response_media_type(mut resp: RtResponse) -> RtResponse { if resp.headers().get(CONTENT_TYPE) == Some(PROTOCOL_MEDIA_TYPE) { resp.headers_mut().insert(CONTENT_TYPE, RESPONSE_MEDIA_TYPE); } resp }`
- the `ErasedOperation::run` bodies match line for line: services/common/src/response_media_type.rs:186-201 vs src/service/response_media_type.rs:124-139 (same `routing_key` lookup, same `routing_error_response(req)` fallback, same re-announce), as do `routes` (shared :142-149 vs local :90-100).

The dependency is already in the build — services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/Cargo.toml:35: `service-common = { path = "../common" }`.

Tests that must stay green and unchanged: the local unit test `only_the_protocol_implied_media_type_is_reannounced` (src/service/response_media_type.rs:180-206), the shared crate's `only_the_implied_media_type_is_reannounced` (services/common/src/response_media_type.rs:244-290), and the integration suite tests/all/response_media_type.rs — `every_routed_response_announces_the_services_media_type` (:117, driving CreateDashboard and GetDashboard successes, a modelled exception and a boundary refusal), `a_response_rendered_before_routing_announces_the_services_media_type` (:232), `a_transport_failure_announces_the_services_media_type` (:259) and `the_authentication_refusal_is_announced_by_the_framework_alone` (:287).

## Steps to Reproduce

1. Read services/common/src/response_media_type.rs:59-73 — the shared module states that this service "states it inline today (src/service/response_media_type.rs)" and is "STILL TO MIGRATE ... including its own copy of ResponseMediaTypeOperation".
2. Diff services/common/src/response_media_type.rs:128-149,179-209 against services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/service/response_media_type.rs:77-100,117-148 — the comparison, the insert, the routing-key lookup, the fallback and auth_schemes are the same code.
3. Confirm the replacement crate is already a dependency: grep -n 'service-common' services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/Cargo.toml -> line 35.
4. Note that both copies carry their own equivalent unit test, so the rule is now asserted twice as well.
