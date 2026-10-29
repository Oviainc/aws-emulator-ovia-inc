Issue-Source: trace-sweep-divergence
Issue-Trace: backup-gateway-import-hypervisor-password-validation
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-gateway-import-hypervisor-password-validation
Issue-Shape-Id: com.amazonaws.backupgateway#BackupOnPremises_v20210101
Issue-Key: backup-gateway|ImportHypervisorConfiguration|2xx->2xx|$.HypervisorArn
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# ImportHypervisorConfiguration mints a HypervisorArn whose resource id is not AWS's hypervisor id form

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportHypervisorConfiguration

## Description
Cause 1 of 1: the HypervisorArn returned by a successful ImportHypervisorConfiguration does not have the resource-id shape AWS mints for a hypervisor.

AWS: recorded behavior only. The Smithy model constrains the member but does not fix the id form: ImportHypervisorConfiguration.HypervisorArn targets ServerArn, @length min 50 max 500, @pattern ^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$, and its documentation says only "The Amazon Resource Name (ARN) of the hypervisor" (smithy_query shape=ServerArn / doc ImportHypervisorConfigurationOutput$HypervisorArn). The recorded AWS response therefore carries the id form as a SHAPE assertion rather than a literal: the fixture records HypervisorArn as '<shaped-not-asserted>', i.e. the concrete id is per-account and not asserted, but the value must still belong to the shape class AWS mints for `hypervisor/<id>` — an opaque service-minted hypervisor id, not a zero-padded sequence counter under a crate-local prefix. The whole ARN is a success-path 200 body member; no status or error code is involved in this divergence.

Emulator: the replayed 200 answered HypervisorArn = arn:aws:backup-gateway:<region>:<account>:hypervisor/hv-<16 hex digits> (the replay step observed .../hypervisor/hv-0000000000000000), which fails the recorded shape assertion on $.HypervisorArn. The id is minted by this crate's own convention and nothing else: state.rs declares HYPERVISOR_ID_PREFIX = "hv-" and minted_resource_id renders `format!("{prefix}{sequence:016x}")`, so the resource id is the literal prefix `hv-` followed by a per-(resource,region) monotonic counter padded to 16 hex digits; minted_resource_arn/mint_resource are the single mint, called from State::import_hypervisor, whose ARN ImportHypervisorConfiguration::handle returns verbatim. Because the counter starts at 0 for each region, the first import of every fresh process returns the SAME id in every run, so the value is predictable rather than opaque. Note the sibling virtual-machine id DOES have model grounding for its form (the VirtualMachine$ResourceArn doc gives `vm/vm-0000ABCDEFGIJKL`); the hypervisor prefix and width have none — they are the crate's invention.

Why: a client that round-trips the ARN is fine, but one that recognises or parses AWS hypervisor identities breaks. HypervisorDetails/VirtualMachine report a bare HypervisorId read off this ARN (state.rs arn_resource_id), so any consumer that validates that id against AWS's hypervisor id form — console/CLI output matching, IAM resource patterns or customer code keyed on the id — rejects or mis-parses everything this emulator mints. Test fixtures and golden files recorded against AWS do not match the emulator, and the deterministic zero-padded counter means two independent emulator runs hand out identical hypervisor identities, so a test that keys state on a hypervisor id cannot tell two runs apart.

Fix: the id/ARN mint for the hypervisor resource in this service's state (the single mint path state.rs::mint_resource / minted_resource_id and the HYPERVISOR_ID_PREFIX it renders), not the operation handler, which only returns the ARN the store minted. The handler and the response serializer need no change once the minted id has AWS's hypervisor id shape (an opaque, non-sequential service-minted id after `hypervisor/`), and the fix must keep the provenance check (is_minted_resource_arn / minted_resource_sequence, which read the id form back) agreeing with whatever form the mint produces, since page tokens are validated through it.

Related but NOT this cause: open issue 20261028T182654Z-backup-gateway-importhypervisorconfiguration-wrong-behavior covers the same minted ARN's REGION segment being taken unbounded from the signing scope (so an over-long ARN can exceed ServerArn's 500 bound). It says nothing about the resource-id form and does not carry this fixture step, so it does not cover this divergence.

## Evidence
Replay step backup-gateway-import-hypervisor-password-validation#task#2, operation ImportHypervisorConfiguration, verdict mismatch.

Expected (recorded AWS): HTTP 200, $.HypervisorArn = '<shaped-not-asserted>' — a hypervisor ARN whose `hypervisor/<id>` id belongs to AWS's minted-hypervisor-id shape class (literal id not asserted because it is per-account).
Actual (emulator): HTTP 200, $.HypervisorArn = 'arn:aws:backup-gateway:us-west-2:000000000000:hypervisor/hv-0000000000000000' — reported by the replay as `$.HypervisorArn: value_mismatch`.

Fixed vs variable parts of the emulator value: the prefix `arn:aws:backup-gateway:` and the resource type `hypervisor/` are fixed and correct; the region and account segments are environment values and are NOT part of this finding; the resource id `hv-0000000000000000` is the defect — `hv-` is a crate constant (state.rs HYPERVISOR_ID_PREFIX) and the 16 hex digits are `format!("{sequence:016x}")` of a per-region counter (state.rs minted_resource_id, called from mint_resource -> minted_resource_arn -> State::import_hypervisor), so the trailing zeros are just "first hypervisor in this region" and the whole id shape, not that one value, is what diverges.

Status, HTTP method and every other member of the response matched; 57 of 68 corpus steps passed, so this is a value-shape divergence on an otherwise working success path.

## Steps to Reproduce
1. Boot the emulator from this checkout with the backup-gateway service mounted.
2. POST / with header `X-Amz-Target: BackupOnPremises_v20210101.ImportHypervisorConfiguration` and body {"Name":"sim2real-hv","Host":"10.0.0.1","Username":"admin","Password":"p@ssw0rd"}, signed for any region.
3. Observe HTTP 200 with HypervisorArn ending in `hypervisor/hv-<16 hex digits>` (the first import of a fresh process in that region returns `hv-0000000000000000`).
4. Compare with the recorded AWS response for the same call: the ARN's resource id is an opaque AWS-minted hypervisor id, not `hv-` + a zero-padded 16-digit counter, so the replay's shape assertion on $.HypervisorArn fails.
5. Restart the emulator and repeat step 2 -> the identical ARN is handed out again, showing the id is a deterministic counter rather than an opaque minted identity.
6. Replay the whole corpus (replay_wire_corpus) and read the cluster for fixture backup-gateway-import-hypervisor-password-validation, step task#2.
