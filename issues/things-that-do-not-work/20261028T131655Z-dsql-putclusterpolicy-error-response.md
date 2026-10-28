Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 930ad079f132337074cf049827d554fb10a3d200

# The shared policy-document check is decided AFTER the idempotency ledger, so a used clientToken turns the document refusal into a 409

_Filed: 2026-10-28T13:16:55Z_
_Operation: PutClusterPolicy | Class: error-response_

> _Note: this issue was identified against the code base at commit `930ad079f132337074cf049827d554fb10a3d200`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutClusterPolicy` consults its `clientToken` ledger before it runs `state::validate_cluster_policy`, while `CreateCluster` — the other entry point for the very same document, through the very same helper — runs the document check first and states that ordering as a rule. As a result the two entry points answer the SAME unacceptable document differently once the token has been used by an earlier, different request: `CreateCluster` answers 400 `ValidationException` ("The policy is not a valid JSON policy document."), `PutClusterPolicy` answers 409 `ConflictException` ("Requests using a client token should supply identical request parameters") and never reports what is wrong with the document.

The crate's rationale for the order is written out on the `CreateCluster`/`UpdateCluster` side: input-constraint checks are decided ahead of the idempotency ledger, "which decide[s] what to do with a request whose input is valid in the first place". `PutClusterPolicy` inverts that for the one check both operations share, and the suite that pins the document verdict across both entry points sends no `clientToken`, so the divergence is uncovered.

## Evidence

The ordering rule this crate states — `services/service-com-amazonaws-dsql-dsql/src/ops/create_cluster.rs:233-238`:
"// A multi-Region configuration's WITNESS Region cannot be the cluster's own
        // Region (this operation's documented constraint) ... is refused with the modelled 400 and nothing is created, ahead
        // of the idempotency ledger and the quota check, which decide what to do with
        // a request whose input is valid in the first place."
and the document check placed by that rule — `src/ops/create_cluster.rs:270-288`: "// A `policy` supplied here is persisted as the cluster's own policy record ... so the document is held to the same rule" followed by `crate::state::validate_cluster_policy(document, &arn, input.bypass_policy_lockout_safety_check)` BEFORE the `clientToken` block at `src/ops/create_cluster.rs:290-339`.

The shared helper claims one verdict for both entry points — `src/state.rs:1836-1842`: "Every operation that WRITES a cluster's policy enforces this here and refuses identically, because the document ends up in one place — the cluster's own policy record, which `GetClusterPolicy` reports — so a document one entry point would refuse cannot be reachable through the other".

The handler that inverts the order — `src/ops/put_cluster_policy.rs:189-238` is the ledger (`TokenVerdict::Mismatch => return Err(PutClusterPolicyErrorModeled::ConflictException(crate::state::ResourceIdentity::cluster(&input.identifier).client_token_mismatch())...)`), and only afterwards, at `src/ops/put_cluster_policy.rs:246-252`:
```
        if let Err(invalid) = crate::state::validate_cluster_policy(
            &input.policy,
            &cluster_arn,
            input.bypass_policy_lockout_safety_check,
        ) {
            return Err(PutClusterPolicyErrorModeled::ValidationException(invalid).into());
        }
```

The rule the suite states, and the gap in its coverage — `tests/all/inline_policy_validation.rs:432-434`: "/// RULE, over the two entry points together: the verdict belongs to the DOCUMENT,
/// so `PutClusterPolicy` answers each row with the same status, error code, reason
/// and sentence `CreateCluster` answers it with." — its calls are `dsql.put_policy(identifier, json!({"policy": case.document.clone()}))` and `fresh.create(json!({"deletionProtectionEnabled": false, "policy": case.document.clone()}))` (lines 445-452): neither carries a `clientToken`, so the recorded-token path is never exercised. The sibling rule the crate does pin for the other input check is `tests/all/multi_region_witness.rs:354-356`: "/// RULE: a `clientToken` does not launder the refusal".

## Steps to Reproduce

1. Create a cluster and wait for the clock so it is ACTIVE.
2. `POST /cluster/{identifier}/policy` with `{"policy": <any valid policy document>, "clientToken": "t"}` — 200, and token `t` is recorded against that request's fingerprint (`src/ops/put_cluster_policy.rs:295-302`).
3. `POST /cluster/{identifier}/policy` with `{"policy": "not a policy document", "clientToken": "t"}`.
4. Observe 409 `ConflictException` "Requests using a client token should supply identical request parameters" — the document is never examined.
5. Run the equivalent pair through the other entry point: `CreateCluster` with `{"deletionProtectionEnabled": false, "policy": <valid document>, "clientToken": "t"}` then `CreateCluster` with `{"deletionProtectionEnabled": false, "policy": "not a policy document", "clientToken": "t"}`. The second answers 400 `ValidationException` with reason `cannotParse` and "The policy is not a valid JSON policy document." — a different status, code, reason and sentence for the same document, which the suite's rule says cannot happen.
