Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__iam__lambda__s3.sam-policies-shorthand-expansion-deny-probed
Issue-Key: cloudformation__dynamodb__iam__lambda__s3.sam-policies-shorthand-expansion-deny-probed|run#27|AssertionError|assert policy_names == ["ProbeRolePolicy0", "ProbeRolePolicy1"], (
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# SAM Policies shorthand is not expanded into inline role policies

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
One net-new root cause in the cloudformation__dynamodb__iam__lambda__s3 group.

Cause 1 of 1: the SAM transform drops the `Policies` shorthand.
AWS: when a `AWS::Serverless::Function` declares `Policies: [...]`, the SAM transform creates the function's execution role with one inline policy per `Policies` entry, named positionally and in declaration order, so the deployed role grants exactly those actions.
Emulator: the role is created with no inline policies at all — `ListRolePolicies` returns `[]` — so the shorthand grants nothing and the deny-probe that should be blocked is not.
Why: every SAM template that uses the policy shorthand deploys a function whose role has no permissions (or, from the caller's point of view, permissions that silently differ from the template), and the authorization behavior of the deployed stack cannot be reproduced.
Fix: the cloudformation SAM transform's Policies expansion (role inline-policy generation).
Traces: cloudformation__dynamodb__iam__lambda__s3.sam-policies-shorthand-expansion-deny-probed

## Evidence
[mismatch] cloudformation__dynamodb__iam__lambda__s3.sam-policies-shorthand-expansion-deny-probed — AssertionError: SAM names one inline policy per Policies entry, positionally and in declaration order; got [].

## Steps to Reproduce
Boot the emulator and replay cloudformation__dynamodb__iam__lambda__s3.sam-policies-shorthand-expansion-deny-probed: it deploys a SAM template whose function declares several Policies entries, then enumerates the generated role's inline policies and probes a denied action.
