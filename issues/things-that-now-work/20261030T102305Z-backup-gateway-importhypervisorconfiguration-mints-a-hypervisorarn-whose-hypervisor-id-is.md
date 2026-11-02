Issue-Source: trace-sweep-divergence
Issue-Trace: backup-gateway-import-hypervisor-password-validation
Issue-Step: task#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-gateway-import-hypervisor-password-validation
Issue-Shape-Id: com.amazonaws.backupgateway#BackupOnPremises_v20210101
Issue-Key: backup-gateway|ImportHypervisorConfiguration|2xx->2xx|$.HypervisorArn
Issue-Commit: ecabe5b80661b5f5cfe26ce4b32c6d6d8effd594

# ImportHypervisorConfiguration mints a HypervisorArn whose hypervisor id is not the form the service mints

> _Note: this issue was identified against the code base at commit `ecabe5b80661b5f5cfe26ce4b32c6d6d8effd594`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportHypervisorConfiguration

## Description
Cause 1 of 1: the `HypervisorArn` a successful `ImportHypervisorConfiguration` returns carries a resource id whose FORM is the emulator's own invention, so the recorded shape assertion on that member fails (fixture `backup-gateway-import-hypervisor-password-validation`, step `task#2`).

AWS: recorded behavior only for the id form. The Smithy model constrains `ImportHypervisorConfigurationOutput$HypervisorArn` -> `com.amazonaws.backupgateway#ServerArn` only as `@length(min: 50, max: 500)` with `@pattern "^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\\/[a-zA-Z-0-9]+$"`, and neither `ServerArn` nor `HypervisorDetails$HypervisorArn` nor `GatewayDetails$HypervisorId` documents an example id (smithy_query query=doc on both says only "The hypervisor ID of the gateway."). The model therefore pins the ARN skeleton (partition, `backup-gateway`, three colon segments, `/<id>`) but NOT the id form; the recorded response is asserted on shape rather than on its literal value, because the region, the account number and the generated id are environment-specific. What the recording pins is the SHAPE of the whole member — the resource-type segment plus the lexical form of the id behind the slash — and the value this emulator serves does not satisfy it even though it satisfies the modelled pattern and length. Hypothesis: the service's hypervisor resource id is minted with the `hype-` prefix and a short (8-character) hexadecimal body, i.e. `arn:<partition>:backup-gateway:<region>:<account>:hypervisor/hype-<8 hex>`, which is the form the documented backup-gateway identifiers take; the recording is the authority for the exact form a fixer must mint.

Emulator: the handler mints the identity through the store's single mint path, which renders every resource id as its kind prefix followed by the per-(kind, region) sequence in 16 lowercase hex digits, starting at zero. For the hypervisor kind the prefix is `hv-`, so the first import of a run answers 200 with `HypervisorArn` = `arn:aws:backup-gateway:<region>:<account>:hypervisor/hv-0000000000000000`. The region and account segments are environment values and are not the finding; the `hv-` prefix and the 16-digit zero-padded sequence body are the emulator's own choice and are what the shaped assertion rejects. The same mint feeds every later read, so the non-conforming id is echoed by `GetHypervisor`, `ListHypervisors`, `UpdateHypervisor` and `VirtualMachine$HypervisorId` (which is read back off the stored ARN), and it is also what the listing page tokens are checked against.

Why: a client that parses the hypervisor identity out of the ARN — to render a console id, to key its own records, to validate against the identifier form the service documents, or to build a `hypervisor/<id>` ARN itself — gets an id of the wrong prefix and the wrong width from this emulator and the right one from AWS. Anything that round-trips the bare id (`GatewayDetails$HypervisorId`, `VirtualMachine$HypervisorId`) carries the same wrong form, so a test suite or a tool pinned to the real identifier form passes against AWS and fails against the emulator for a reason that has nothing to do with the behavior under test.

Fix: the ARN/id mint in this service's state — the single rendering point for a minted resource id and the hypervisor id-prefix constant beside it — not the operation handler, which already reads the ARN back out of the store after the insert. Changing the mint is enough to correct the import response and every read that echoes it, and the inverse reader that recognises a minted id (used by the page-token provenance check) must be moved with it so a resumed listing boundary is still recognised.

Traces: `backup-gateway-import-hypervisor-password-validation#task#2` — expected 200 with a shape-asserted `HypervisorArn`, got 200 with `arn:aws:backup-gateway:us-west-2:000000000000:hypervisor/hv-0000000000000000` (`$.HypervisorArn: value_mismatch`).

Related but distinct open issue, which does NOT cover this: `20261028T182654Z-backup-gateway-importhypervisorconfiguration-wrong-behavior.md` reports that the same mint takes an unbounded signing region, so a long region can push the minted ARN past the modelled 500-character ceiling. That is about the REGION segment's length; this finding is about the form of the resource id behind the slash, and fixing either one leaves the other standing.

## Evidence
Replay of fixture `backup-gateway-import-hypervisor-password-validation`, step `task#2` (operation `ImportHypervisorConfiguration`, verdict `mismatch`, expected status 200, actual status 200):

    $.HypervisorArn: value_mismatch (expected '<shaped-not-asserted>', got 'arn:aws:backup-gateway:us-west-2:000000000000:hypervisor/hv-0000000000000000')

The status, the operation and every other member agree; the sole divergence is the value of `HypervisorArn`, whose recorded value is shape-asserted (it is environment-specific) and whose emulator value fails that shape.

Model, for what IS pinned (services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/smithy-model/com-amazonaws-backupgateway-backuponpremises-v20210101.json):

    ImportHypervisorConfigurationOutput$HypervisorArn -> com.amazonaws.backupgateway#ServerArn
      @length(min: 50, max: 500)
      @pattern ^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$

The served value satisfies both (76 characters; three colon segments after `backup-gateway`; an `[a-zA-Z-0-9]+` id), which is why nothing in the emulator refuses it — the divergence is in the id form the model does not express.

Where the id form comes from, in the checkout:

* `services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/src/ops/import_hypervisor_configuration.rs:213-226` — the handler calls `store.import_hypervisor(&region, ...)` and returns `imported.arn` verbatim.
* `.../src/state.rs:1864-1878` (`import_hypervisor`) — `let (_, arn) = self.mint_resource(MintedResource::Hypervisor, region);`
* `.../src/state.rs:2450-2465` (`mint_resource`) -> `.../src/state.rs:2805-2815` (`minted_resource_arn`) -> `.../src/state.rs:2794-2796`:

    fn minted_resource_id(prefix: &str, sequence: u64) -> String {
        format!("{prefix}{sequence:016x}")
    }

* `.../src/state.rs:190` — `const HYPERVISOR_ID_PREFIX: &str = "hv-";`
* `.../src/state.rs:2823-2827` (`minted_resource_sequence`) — the strict inverse reader that the page-token provenance check relies on, so it has to change with the mint.

First import of a run has sequence 0, hence the all-zero body `hv-0000000000000000`; the zeros are a consequence of the sequence, and the prefix plus the 16-digit width are the invariant part of the divergence (a second import in the same region answers `hv-0000000000000001`, which fails the same shape).

## Steps to Reproduce
1. Start the emulator with the backup-gateway service mounted (`/__admin/health` answers 200).
2. POST `/` with `X-Amz-Target: BackupOnPremises_v20210101.ImportHypervisorConfiguration`, a SigV4 Authorization header whose credential scope names any ordinary region, and body `{"Name":"hv-import-1","Host":"10.0.0.1","Username":"admin","Password":"p4ssw0rd"}`.
3. Observe HTTP 200 whose `HypervisorArn` ends in `:hypervisor/hv-0000000000000000` — prefix `hv-` and a 16-hex-digit body, not the hypervisor id form the recorded response is shape-asserted against.
4. Repeat the import: the next ARN ends `:hypervisor/hv-0000000000000001`, confirming the form (prefix + 16-digit zero-padded sequence) rather than the particular zeros is what diverges.
5. Call GetHypervisor/ListHypervisors with that ARN: the same id form is echoed back, and `ListVirtualMachines`/`GetGateway` report the bare id (`HypervisorId`) in the same form.
6. Re-replay fixture `backup-gateway-import-hypervisor-password-validation`, step `task#2` to confirm: the step must pass with the member shape-asserted rather than reporting `$.HypervisorArn: value_mismatch`.

## Resolution
FIXED in the mint, as the report asks: the width of a minted id is now a property of the
RESOURCE KIND beside its prefix (`MintedResource::id_body_digits` in `src/state.rs`), and
the hypervisor kind's form is `hypervisor/hype-<8 lowercase hex>`. `minted_resource_id`
and its strict inverse `minted_resource_sequence` both take the kind, so the mint and the
page-token provenance check that recognises a minted id cannot drift apart, and the mint
stays the deterministic per-(kind, region) sequence every suite rests on. No handler
changed: the import already returns the ARN the store minted, so every surface that echoes
the identity or the bare id inside it (`GetHypervisor`, `ListHypervisors`,
`UpdateHypervisor`, `GatewayDetails$HypervisorId`, `VirtualMachine$HypervisorId`, the
tagging `ResourceARN` and every listing's `NextToken`) carries the corrected form.

The gateway (`gateway/bgw-<16 hex>`) and virtual-machine (`vm/vm-<16 hex>`) forms are
unchanged: nothing published pins their width and no recorded response diverges on them.

The rule is asserted in `tests/all/minted_resource_id_form.rs` over every kind this
service mints, several successive mints of each, in two regions: each id is its kind's
prefix and exactly its kind's width of lowercase hex digits, matches no other kind's form,
is unique in its region, and is what every surface reports.
