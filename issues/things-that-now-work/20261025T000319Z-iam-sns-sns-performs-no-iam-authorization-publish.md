Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__sns.xsvc-iam-2-sns-identity-policy-only-authorizes-publish iam__sns.xsvc-iam-2-sns-topic-policy-deny-beats-identity-allow-denied
Issue-Key: iam__sns.xsvc-iam-2-sns-identity-policy-only-authorizes-publish|_settle#9|Timeout|time.sleep(interval_s)
Issue-Key: iam__sns.xsvc-iam-2-sns-topic-policy-deny-beats-identity-allow-denied|_settle#14|Timeout|time.sleep(interval_s)
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# iam__sns: SNS performs no IAM authorization — Publish by a principal with no identity policy succeeds, so both identity-policy and topic-policy-deny checks hang to the 60 s cap

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 1: SNS never evaluates the caller's IAM identity policy or the topic's resource policy, so no Publish is ever refused.
AWS: an SNS Publish is authorized by the union of the caller's IAM identity policy and the topic policy. A fresh IAM user with NO identity policy publishing to a topic that carries only AWS's default policy (Id '__default_policy_ID') is refused with exactly error code AuthorizationError; after PutUserPolicy adds a single sns:Publish statement for that topic ARN the same signed Publish returns a MessageId. Conversely an explicit Deny in the topic policy beats an identity-policy Allow and the Publish is refused again.
Emulator: the signed Publish keeps succeeding in the state where AWS refuses it, so the checks' settle loop polls for the AuthorizationError/denial that never appears and both die at 'Check timed out after 60.0s'.
Why: every least-privilege assertion about SNS passes vacuously here — a topic a client believes is locked down accepts messages from any principal, and the deny-beats-allow precedence that clients rely on for guardrails is unobservable. A deployment validated against this emulator is unsafe in production.
Fix: an authorization gate ahead of the SNS operation handlers in the sns service crate: resolve the signing principal, evaluate its IAM identity/inline policies together with the topic's Policy attribute (explicit Deny wins), and refuse with AuthorizationError.
Note: the open issue 20261022T220446Z-iam-no-iam-authorization-is-evaluated-calls-a-session.md records the same class of gap for IAM's own API; these two checks were not attributed to it and cover the SNS data plane.
Traces: iam__sns.xsvc-iam-2-sns-identity-policy-only-authorizes-publish, iam__sns.xsvc-iam-2-sns-topic-policy-deny-beats-identity-allow-denied.

## Evidence
Both checks: Error: Check timed out after 60.0s — each polls a signed sns:Publish for the expected flip. Expected, per the checks' recorded contract: error code 'AuthorizationError' while the user has no identity policy (topic keeps Policy Id '__default_policy_ID'), a MessageId after PutUserPolicy grants sns:Publish on that topic ARN, and 'AuthorizationError' again once the topic policy explicitly denies the principal.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. CreateTopic (leave its default policy in place), create an IAM user with no policies and one access key.
3. Publish to the topic signed with that key — observe success instead of AuthorizationError.
4. PutUserPolicy granting sns:Publish on the topic ARN, publish again (succeeds), then SetTopicAttributes with a policy that explicitly denies that principal and publish again — observe success instead of AuthorizationError.
5. Re-replay the two check ids in trace_ids to confirm.
