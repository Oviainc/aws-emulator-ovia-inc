Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 6b1652c669c36440eab65de28fac2314531b273d

# Reject a JSON object that is not a valid resource policy document

_Filed: 2026-10-24T20:08:26Z_
_Operation: PutContactPolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `6b1652c669c36440eab65de28fac2314531b273d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler accepts any `Policy` string that parses as a JSON object. A document with no `Version`/`Statement` — e.g. `{"foo": 1}` — or a statement with no `Effect`/`Principal`/`Action` is persisted and returned verbatim by `GetContactPolicy`. The real service parses the resource policy and refuses a document that is not a well-formed policy with `ValidationException` (400).

Expected: `PutContactPolicy` with a JSON object that is not a valid policy document is refused with the modelled `ValidationException` (400) and the contact keeps whatever policy it had.
Actual: HTTP 200 and the garbage document becomes the contact's policy, readable through `GetContactPolicy`.

## Evidence

Spec: `PutContactPolicy` declares `ValidationException` (400) and takes `Policy` (len 1..395000, pattern `\S`) described as the contact's resource policy; `GetContactPolicy` reports it back (OPERATIONS.md, PutContactPolicy/GetContactPolicy). Well-established AWS behaviour: resource-policy APIs validate the policy document (Version/Statement/Effect/Principal/Action), not merely that the body is JSON.

Handler: `src/ops/put_contact_policy.rs` lines 191-199 — the only check is `serde_json::from_str::<serde_json::Value>(&input.policy).is_ok_and(|document| document.is_object())`; the string is then stored as-is (line 212).

Tests: `tests/all/data_plane_precondition.rs` exercises the policy slice for the account gate; no test asserts a refusal for a JSON object that is not a policy document.

## Steps to Reproduce

1. Onboard the account and CreateContact to get a ContactArn.
2. PutContactPolicy with ContactArn=<arn> and Policy="{\"foo\": 1}" — expect ValidationException (400); actual: 200.
3. GetContactPolicy with the same ContactArn — the non-policy document is returned as the contact's policy.
