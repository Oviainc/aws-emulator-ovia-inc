Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.create-fpga-image-invalid-bucket-rejected ec2.op-contract-delete-fpga-image-rejected ec2.op-contract-reset-fpga-image-attribute ec2.op-contract-attachImageWatermark ec2.op-contract-delete-image-usage-report ec2.op-contract-create-restore-image-task-rejected ec2.op-contract-list-volumes-in-recycle-bin ec2.op-contract-enable-volume-io ec2.detach-volume-not-attached-state-conflict-rejected ec2.op-contract-cancel-export-task ec2.image-task-and-fpga-error-semantics ec2.instance-task-error-semantics
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.create-fpga-image-invalid-bucket-rejected|run#5|AssertionError|assert bucket_code == "InaccessibleStorageLocation", (
Issue-Key: ec2.op-contract-delete-fpga-image-rejected|run#4|AssertionError|assert dry_run_code == "DryRunOperation", f"expected DryRunOperation, got {dry_run_code!r}"
Issue-Key: ec2.op-contract-reset-fpga-image-attribute|run#3|AssertionError|assert malformed_code == "InvalidFpgaImageID.Malformed", (
Issue-Key: ec2.op-contract-attachImageWatermark|run#4|AssertionError|assert code == "InvalidAMIID.Malformed", f"expected InvalidAMIID.Malformed, got {code}"
Issue-Key: ec2.op-contract-delete-image-usage-report|run#4|AssertionError|assert dry_run_code == "DryRunOperation", f"expected DryRunOperation, got {dry_run_code!r}"
Issue-Key: ec2.op-contract-create-restore-image-task-rejected|run#5|AssertionError|assert dry_run_code == "DryRunOperation", f"expected DryRunOperation, got {dry_run_code!r}"
Issue-Key: ec2.op-contract-list-volumes-in-recycle-bin|run#15|AssertionError|assert code == "DryRunOperation", f"expected DryRunOperation, got {code!r}"
Issue-Key: ec2.op-contract-enable-volume-io|run#7|AssertionError|assert code == "IncorrectState", f"expected IncorrectState, got {code!r}"
Issue-Key: ec2.detach-volume-not-attached-state-conflict-rejected|run#7|AssertionError|assert code == "IncorrectState", code
Issue-Key: ec2.op-contract-cancel-export-task|run#3|AssertionError|assert code == "InvalidExportTaskID.NotFound", f"expected InvalidExportTaskID.NotFound, got {code}"
Issue-Key: ec2.image-task-and-fpga-error-semantics|run#4|AssertionError|assert not problems, "unexpected image/FPGA error semantics:\n" + "\n".join(problems)
Issue-Key: ec2.instance-task-error-semantics|run#9|AssertionError|assert not problems, "unexpected instance-task error semantics:\n" + "\n".join(problems)
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# EC2 FPGA/AMI task, recycle-bin, volume-IO and export-task operations answer 501 NotImplemented instead of their modeled refusals (10 causes)

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFpgaImage

## Description
Ten unimplemented EC2 operations, each its own cause. All ten must be closed before submitting. Common shape: these operations model a DryRun member and no error shapes (Smithy com-amazonaws-ec2-amazonec2; EC2 error codes are ec2Query wire codes, documented not modeled). AWS answers an authorized DryRun=true call with 400 DryRunOperation 'Request would have succeeded, but DryRun flag is set.', a malformed id with the operation's .Malformed code, and a wrong-state request with IncorrectState. The emulator answers NotImplemented ('operation <Op> is scaffolded but not implemented in this emulator'). Why, for all ten: the FPGA-image, AMI-task, recycle-bin and volume-IO surfaces are unusable and none of their refusal classes is observable, so a client cannot build or negatively test any of these flows. Fix, for all ten: implement the named handler under services/service-com-amazonaws-ec2-amazonec2/src/ops/.

Cause 1 of 10: CreateFpgaImage — AWS refuses a malformed S3 bucket name with InaccessibleStorageLocation; got NotImplemented. Trace: ec2.create-fpga-image-invalid-bucket-rejected.
Cause 2 of 10: DeleteFpgaImage — expected DryRunOperation, got NotImplemented. Trace: ec2.op-contract-delete-fpga-image-rejected.
Cause 3 of 10: ResetFpgaImageAttribute — expected InvalidFpgaImageID.Malformed, got NotImplemented. Trace: ec2.op-contract-reset-fpga-image-attribute.
Cause 4 of 10: AttachImageWatermark — expected InvalidAMIID.Malformed, got NotImplemented. Trace: ec2.op-contract-attachImageWatermark.
Cause 5 of 10: DeleteImageUsageReport — expected DryRunOperation, got NotImplemented. Trace: ec2.op-contract-delete-image-usage-report.
Cause 6 of 10: CreateRestoreImageTask — expected DryRunOperation, got NotImplemented. Trace: ec2.op-contract-create-restore-image-task-rejected.
Cause 7 of 10: ListVolumesInRecycleBin — expected DryRunOperation, got NotImplemented. Trace: ec2.op-contract-list-volumes-in-recycle-bin.
Cause 8 of 10: EnableVolumeIO — expected IncorrectState for a volume not in an error state, got NotImplemented. Trace: ec2.op-contract-enable-volume-io.
Cause 9 of 10: the volume detach path — AWS refuses detaching a volume that is not attached with IncorrectState; the emulator answers NotImplemented. Trace: ec2.detach-volume-not-attached-state-conflict-rejected.
Cause 10 of 10: CancelExportTask — expected InvalidExportTaskID.NotFound, got NotImplemented. Trace: ec2.op-contract-cancel-export-task.

The aggregate family checks ec2.image-task-and-fpga-error-semantics and ec2.instance-task-error-semantics fail over this same surface and are listed in trace_ids so they are re-replayed with the fix.

## Evidence
Replay output: AssertionError: expected CreateFpgaImage to reject a malformed S3 bucket name with InaccessibleStorageLocation, got 'NotImplemented'; AssertionError: expected DryRunOperation, got 'NotImplemented' (causes 2,5,6,7); AssertionError: expected InvalidFpgaImageID.Malformed, got NotImplemented; AssertionError: expected InvalidAMIID.Malformed, got NotImplemented; AssertionError: expected IncorrectState, got 'NotImplemented'; AssertionError: NotImplemented (detach-volume); AssertionError: expected InvalidExportTaskID.NotFound, got NotImplemented. Every NotImplemented body reads 'operation <Op> is scaffolded but not implemented in this emulator'.

## Steps to Reproduce
Replay the check ids in trace_ids. Minimal repro per cause: call the named operation with DryRun=True (expect DryRunOperation) and with a malformed/wrong-state target (expect the Malformed/IncorrectState code); observe NotImplemented.
