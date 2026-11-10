Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-account-configuration-roundtrip
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-account-configuration-roundtrip
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|DescribeAccountConfiguration|2xx->2xx|$.accountConfiguration.kmsKeyId
Issue-Commit: 1e71a3102b418a18abda1ed5ba2c04b37249c268

# DescribeAccountConfiguration reports a synthesized aws/medialive alias instead of the kmsKeyId the account configured

> _Note: this issue was identified against the code base at commit `1e71a3102b418a18abda1ed5ba2c04b37249c268`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAccountConfiguration

## Description
One cause.

Cause 1 of 1 — the account configuration a caller reads back is not the one the account wrote; it is a constant the emulator synthesizes.
AWS: UpdateAccountConfiguration (Smithy: PUT /prod/accountConfiguration, input and output AccountConfiguration) and DescribeAccountConfiguration (Smithy: GET /prod/accountConfiguration, output AccountConfiguration) are a write/read pair over one account-level setting. AccountConfiguration has exactly one member, KmsKeyId (jsonName `kmsKeyId`), documented as "Specifies the KMS key to use for all features that use key encryption. Specify the ARN of a KMS key that you have created. Or leave blank to use the key that MediaLive creates and manages for you." So the member carries the CALLER'S OWN customer-managed key ARN: Describe reports back whatever the last successful Update stored for the account, and when nothing has been configured the member is blank/absent rather than filled in with MediaLive's managed key. Neither the model nor the recording says the service ever reports its own managed key here, and MediaLive's managed key is not something a caller can address as a configured kmsKeyId. The recording for this roundtrip asserts, at the read step, the key ARN the preceding write established for the account (the fixture parameterizes it as the account's baseline key, since the ARN embeds the account id and key id — those variable parts are the account's, not fixed text).
Emulator: 200, but $.accountConfiguration.kmsKeyId is `arn:aws:kms:us-west-2:000000000000:alias/aws/medialive` — a value tied to no request: the all-zero account id and the `alias/aws/medialive` ALIAS path are both emulator inventions (an alias ARN is not a key ARN, and `000000000000` is not the caller's account). So the value read back is independent of what the roundtrip wrote: the configured key is not persisted into, or not read out of, the account-level configuration slice.
Why: this setting is how a client pins customer-managed encryption for MediaLive features. A client that writes its key and reads back to confirm (the normal confirm-after-write, and the way infrastructure tooling detects drift) sees a key it never set, concludes the write did not take, and either rewrites forever or reports the account as misconfigured. Worse, the value it reads is unusable: passing `arn:aws:kms:us-west-2:000000000000:alias/aws/medialive` to KMS or back into UpdateAccountConfiguration names nothing in the caller's account, so any code that round-trips the describe output into a later call is broken. A caller that has configured nothing is also told encryption is pinned to a key when AWS would report the member blank.
Hypothesis for the mechanism (not stated by model or recording): the crate holds the account configuration beside the region partitions keyed by the region the writing request was signed for (described in src/state/regions.rs and src/state/account_configuration.rs, quoted in the open issue 20261029T182501Z-medialive-listversions-wrong-behavior.md), so either the write is not stored on that account-level slice at all or the read falls back to a hardcoded default instead of the stored value.
Fix: the account-configuration state slice plus the DescribeAccountConfiguration/UpdateAccountConfiguration handlers — Describe must project the stored AccountConfiguration for the signed account/region (the exact kmsKeyId string the last Update accepted, byte for byte) and must OMIT kmsKeyId when no Update has set one. No default, alias-form or account-placeholder value may be substituted.
Traces: medialive-account-configuration-roundtrip#task#2.

Not covered by any open issue: no filed medialive issue names DescribeAccountConfiguration or this replay key; 20261029T182501Z-medialive-listversions-wrong-behavior.md only cites the account-configuration slice as a contrast for where the engine-version catalog lives.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 97/113 steps passed). Cluster, verbatim:

  fixture_id: medialive-account-configuration-roundtrip
  step_key:   task#2
  operation:  DescribeAccountConfiguration
  verdict:    mismatch, expected_status 200, actual_status 200
  detail:     $.accountConfiguration.kmsKeyId: value_mismatch (expected '{{baseline_kms_key_id}}', got 'arn:aws:kms:us-west-2:000000000000:alias/aws/medialive')

The expectation is the roundtrip's own configured key (the fixture parameterizes the account-specific ARN); the requirement under test is that the read reflects the write. What the emulator returned reflects neither the request nor any stored value: `000000000000` is the emulator's placeholder account and `alias/aws/medialive` is an alias path, not a KMS key ARN, so the response is a hardcoded constant rather than the configured kmsKeyId. Smithy: AccountConfiguration$KmsKeyId (jsonName kmsKeyId, target __string) — "Specify the ARN of a KMS key that you have created. Or leave blank to use the key that MediaLive creates and manages for you"; UpdateAccountConfiguration PUT /prod/accountConfiguration and DescribeAccountConfiguration GET /prod/accountConfiguration share the AccountConfiguration shape on input and output.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an AWS SDK/CLI for medialive at it, signed for us-west-2.
2. GET /prod/accountConfiguration (aws medialive describe-account-configuration) on a fresh store: the emulator answers 200 with accountConfiguration.kmsKeyId = `arn:aws:kms:us-west-2:000000000000:alias/aws/medialive`, where AWS reports the member blank/absent for an account that has configured no key.
3. PUT /prod/accountConfiguration (aws medialive update-account-configuration --account-configuration kmsKeyId=arn:aws:kms:us-west-2:<your-account>:key/<key-id>) and note the 200 response.
4. GET /prod/accountConfiguration again: the kmsKeyId reported is still the same synthesized `...:000000000000:alias/aws/medialive` constant rather than the ARN just written, so the write/read roundtrip does not hold.
5. Replay fixture medialive-account-configuration-roundtrip step task#2 to confirm; the fixture's other steps and the 97 currently passing steps must stay passing.
