Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.accept-reserved-instances-exchange-quote-stale-quote-rejected ec2.op-contract-acceptReservedInstancesExchangeQuote-b ec2.op-contract-get-reserved-instances-exchange-quote ec2.op-contract-get-capacity-reservation-usage-dup ec2.op-contract-get-groups-for-capacity-reservation-dup ec2.op-contract-move-capacity-reservation-instances-rejected ec2.op-contract-disable-capacity-manager ec2.get-console-output-invalid-instance-rejected-dup ec2.get-console-screenshot-invalid-instance-rejected-dup ec2.get-host-reservation-purchase-preview-invalid-host-rejected ec2.op-contract-cancel-declarative-policies-report ec2.op-contract-get-declarative-policies-report-summary ec2.capacity-reservation-family-error-semantics
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.accept-reserved-instances-exchange-quote-stale-quote-rejected|run#4|AssertionError|assert get_info["code"] == "InvalidReservedInstanceId", (
Issue-Key: ec2.op-contract-acceptReservedInstancesExchangeQuote-b|run#5|AssertionError|assert info["code"] == "DryRunOperation", f"expected DryRunOperation, got {info['code']!r}"
Issue-Key: ec2.op-contract-get-reserved-instances-exchange-quote|run#4|AssertionError|assert code == "InvalidReservedInstanceId", f"unexpected error code: {code}"
Issue-Key: ec2.op-contract-get-capacity-reservation-usage-dup|run#4|AssertionError|assert dry_run_code == "DryRunOperation", f"expected DryRunOperation, got {dry_run_code!r}"
Issue-Key: ec2.op-contract-get-groups-for-capacity-reservation-dup|run#4|AssertionError|assert dry_run_code == "DryRunOperation", f"expected DryRunOperation, got {dry_run_code!r}"
Issue-Key: ec2.op-contract-move-capacity-reservation-instances-rejected|run#3|AssertionError|assert err == "InvalidCapacityReservationId.Malformed", (
Issue-Key: ec2.op-contract-disable-capacity-manager|run#3|AssertionError|assert code == "CapacityManager.Disabled", (
Issue-Key: ec2.get-console-output-invalid-instance-rejected-dup|run#4|AssertionError|assert dry_run_code == "DryRunOperation", f"expected DryRunOperation, got {dry_run_code!r}"
Issue-Key: ec2.get-console-screenshot-invalid-instance-rejected-dup|run#4|AssertionError|assert dry_run_code == "DryRunOperation", f"expected DryRunOperation, got {dry_run_code!r}"
Issue-Key: ec2.get-host-reservation-purchase-preview-invalid-host-rejected|run#4|AssertionError|assert code == "InvalidHostId.Malformed", f"expected InvalidHostId.Malformed, got {code!r}"
Issue-Key: ec2.op-contract-cancel-declarative-policies-report|run#3|AssertionError|assert malformed_code == "InvalidDeclarativePoliciesReportId.Malformed", (
Issue-Key: ec2.op-contract-get-declarative-policies-report-summary|run#4|AssertionError|assert dry_run_code == "DryRunOperation", f"expected DryRunOperation, got {dry_run_code!r}"
Issue-Key: ec2.capacity-reservation-family-error-semantics|run#5|AssertionError|assert not problems, (
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# EC2 reserved-instance exchange, capacity-reservation usage, dedicated-host preview, console and declarative-policy-report operations answer 501 NotImplemented (10 causes)

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AcceptReservedInstancesExchangeQuote

## Description
Ten unimplemented EC2 operations, each its own cause. All ten must be closed before submitting. Common shape: every one models a DryRun member and no error shapes (Smithy com-amazonaws-ec2-amazonec2: e.g. GetConsoleOutput input InstanceId required + DryRun: Boolean, errors: []; EC2 error codes are ec2Query wire codes, documented not modeled). AWS answers an authorized DryRun=true call with 400 DryRunOperation 'Request would have succeeded, but DryRun flag is set.' and an unowned/unknown id with that operation's own code. The emulator answers NotImplemented ('operation <Op> is scaffolded but not implemented in this emulator'). Why, for all ten: purchasing/exchange, capacity-reservation reporting, dedicated-host cost preview and instance-console diagnostics are entirely unavailable, and their refusal codes cannot be exercised. Fix, for all ten: implement the named handler under services/service-com-amazonaws-ec2-amazonec2/src/ops/.

Cause 1 of 10: AcceptReservedInstancesExchangeQuote — AWS gives DryRunOperation under DryRun and InvalidReservedInstanceId for an unowned, well-formed RI id; got NotImplemented. Traces: ec2.accept-reserved-instances-exchange-quote-stale-quote-rejected, ec2.op-contract-acceptReservedInstancesExchangeQuote-b.
Cause 2 of 10: GetReservedInstancesExchangeQuote — 'unexpected error code: NotImplemented'. Trace: ec2.op-contract-get-reserved-instances-exchange-quote.
Cause 3 of 10: GetCapacityReservationUsage — expected DryRunOperation, got NotImplemented. Trace: ec2.op-contract-get-capacity-reservation-usage-dup.
Cause 4 of 10: GetGroupsForCapacityReservation — expected DryRunOperation, got NotImplemented. Trace: ec2.op-contract-get-groups-for-capacity-reservation-dup.
Cause 5 of 10: MoveCapacityReservationInstances — expected InvalidCapacityReservationId.Malformed, got NotImplemented. Trace: ec2.op-contract-move-capacity-reservation-instances-rejected.
Cause 6 of 10: DisableCapacityManager — expected CapacityManager.Disabled for a not-yet-enabled account, got NotImplemented. Trace: ec2.op-contract-disable-capacity-manager.
Cause 7 of 10: GetConsoleOutput — expected DryRunOperation, got NotImplemented. Trace: ec2.get-console-output-invalid-instance-rejected-dup.
Cause 8 of 10: GetConsoleScreenshot — expected DryRunOperation, got NotImplemented. Trace: ec2.get-console-screenshot-invalid-instance-rejected-dup.
Cause 9 of 10: GetHostReservationPurchasePreview — expected InvalidHostId.Malformed, got NotImplemented. Trace: ec2.get-host-reservation-purchase-preview-invalid-host-rejected.
Cause 10 of 10: the declarative-policies report surface (CancelDeclarativePoliciesReport, GetDeclarativePoliciesReportSummary) — expected InvalidDeclarativePoliciesReportId.Malformed and DryRunOperation respectively, both got NotImplemented. Traces: ec2.op-contract-cancel-declarative-policies-report, ec2.op-contract-get-declarative-policies-report-summary.

The aggregate family check ec2.capacity-reservation-family-error-semantics fails over this same surface and is listed in trace_ids so it is re-replayed with the fix.

## Evidence
Replay output: AssertionError: expected InvalidReservedInstanceId, got 'NotImplemented'; AssertionError: expected DryRunOperation, got 'NotImplemented' (causes 1,3,4,7,8,10); AssertionError: unexpected error code: NotImplemented; AssertionError: expected InvalidCapacityReservationId.Malformed, got NotImplemented; AssertionError: expected CapacityManager.Disabled, got NotImplemented; AssertionError: expected InvalidHostId.Malformed, got 'NotImplemented'; AssertionError: expected InvalidDeclarativePoliciesReportId.Malformed, got NotImplemented. Every NotImplemented body reads 'operation <Op> is scaffolded but not implemented in this emulator'. Expected: HTTP 400 with the named ec2Query code, or DryRunOperation 'Request would have succeeded, but DryRun flag is set.'.

## Steps to Reproduce
Replay the check ids in trace_ids. Minimal repro per cause: call the named operation once with DryRun=True (expect DryRunOperation) and once with a well-formed but nonexistent/unowned id (expect the operation's Malformed/NotFound code); observe NotImplemented in both arms.
