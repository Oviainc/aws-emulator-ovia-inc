Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 7959e6ca7cb81d5c32ad32f8e1e29786f97ad065

# The policy-text-to-document parse and depth refusal is copied into five policy handlers

_Filed: 2026-10-28T10:01:50Z_
_Operation: CreateSecurityPolicy | Class: maintainability_

> _Note: this issue was identified against the code base at commit `7959e6ca7cb81d5c32ad32f8e1e29786f97ad065`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

One rule — "parse the request's `policy` text as JSON, refuse with `ValidationException` naming the parse error, then convert it with `document_from_json` and refuse with `ValidationException` naming `MAX_DOCUMENT_DEPTH`" — is written out five times, once per policy-writing handler (CreateSecurityPolicy, UpdateSecurityPolicy, CreateAccessPolicy, UpdateAccessPolicy, CreateLifecyclePolicy), including the two message templates. The copies can drift: a change to either message, or to the bound that is named, has to be made in five places, and the crate already concentrates the merge half of the same rule in one place (`PolicyWrite::apply`, "the ONE place the merge rule lives, so a data access policy and a security policy cannot drift apart"). A single helper in `crate::state` returning `Result<Document, String>` (the message each handler already wraps in its own error enum) would remove the duplication without changing any response byte.

## Evidence

Copy 1 — src/ops/create_security_policy.rs:189-206: "let parsed: serde_json::Value = serde_json::from_str(&input.policy).map_err(|error| {\n            CreateSecurityPolicyErrorModeled::ValidationException(ValidationException {\n                message: Some(format!(\"policy must be a JSON document: {error}\")),\n            })\n        })?;" … "let Some(policy) = crate::state::document_from_json(&parsed) else { … format!(\"policy must not nest deeper than {} levels\", crate::state::MAX_DOCUMENT_DEPTH) … }".

Copy 2 — src/ops/create_lifecycle_policy.rs:204-239: the same two steps with the same two messages ("policy must be a JSON document: {error}", "policy must not nest deeper than {} levels"), differing only in the error enum and the audit call.

Copies 3-5 — src/ops/create_access_policy.rs:203-238, src/ops/update_access_policy.rs:206-247, src/ops/update_security_policy.rs:196-219.

The crate's own stated preference for one place per rule: "/// Apply this write to one stored policy's mutable members: the ONE place the merge\n    /// rule lives, so a data access policy and a security policy cannot drift apart." — src/state.rs:1032-1033.

Tests that pin the behaviour and must stay unchanged: tests/all/collection_lifecycle.rs:1193-1206 (CreateSecurityPolicy with `"policy": "not json at all"`), tests/all/access_policy.rs:288-301 and :730 (CreateAccessPolicy with the same text), tests/all/lifecycle_policy.rs:497 (`("not JSON at all", json!("not json at all"))`).

## Steps to Reproduce

1. grep -n "policy must be a JSON document" services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/ → five handlers carry the same parse-and-refuse block.
2. grep -n "policy must not nest deeper than" in the same tree → the same five handlers carry the same depth refusal.
3. Change either message or the bound it names and observe that five handler copies must be edited in step for the responses to stay consistent.
