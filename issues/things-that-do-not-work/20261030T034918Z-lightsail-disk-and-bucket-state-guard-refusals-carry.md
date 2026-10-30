Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lightsail.disk-attach-already-attached-rejected lightsail.disk-delete-while-attached-rejected lightsail.bucket-delete-nonempty-without-force-rejected
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail.disk-attach-already-attached-rejected|run#20|AssertionError|assert message == "You can't attach this disk right now. The state of this disk is: in-use.", (
Issue-Key: lightsail.disk-delete-while-attached-rejected|run#18|AssertionError|assert message == (
Issue-Key: lightsail.bucket-delete-nonempty-without-force-rejected|run#12|AssertionError|assert message == f"Bucket is not empty: {bucket_name}", f"unexpected message deleting nonvacated bucket: {message!r}"
Issue-Commit: f4d2c47849283505bef5d66ed0dfe45248bd135a

# Lightsail disk and bucket state-guard refusals carry non-AWS message text (AttachDisk, DeleteDisk, DeleteBucket)

> _Note: this issue was identified against the code base at commit `f4d2c47849283505bef5d66ed0dfe45248bd135a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachDisk

## Description
Three net-new root causes, one family: the refusals the emulator serves when a Lightsail resource's state blocks a well-formed call carry the right HTTP status and the right modelled error, but message text AWS never emits. Lightsail is awsJson1_1 and its error structures (OperationFailureException, InvalidInputException) model members code/docs/message/tip, so `message` is part of the wire contract clients and the console read, not cosmetic prose. Each cause lives in its own handler and must be closed separately.

Cause 1 of 3: AttachDisk on a disk already attached to another instance refuses with the emulator's own sentence instead of AWS's state sentence.
AWS: Smithy (com.amazonaws.lightsail#AttachDisk) — input diskName/instanceName (ResourceName, required, pattern ^\w[\w\-]*\w$) and diskPath (NonEmptyString, required); modeled error OperationFailureException (HTTP 400), documented "Lightsail throws this exception when an operation fails to execute". Recorded behavior: the re-attach of an in-use disk is refused with code OperationFailureException and message "You can't attach this disk right now. The state of this disk is: in-use." Fixed text: "You can't attach this disk right now. The state of this disk is: " + <state> + "."; the only variable part is the disk's current DiskState value (here `in-use`) taken from the emulator's state — AWS keys the sentence on the DISK STATE and names no instance in it.
Emulator: same status and code, but message "The disk <diskName> is already attached to the instance <instanceName>. Detach the disk before attaching it to another instance." — a sentence keyed on the attachment and naming both resources, which AWS does not produce.
Why: a client (or test, or console surface) that recognises the state-blocked condition from AWS's sentence — and anything that parses the reported state out of it — never matches, so an expected, recoverable "disk busy" is treated as an unknown operation failure.
Fix: the AttachDisk handler's attachment guard (the `attached_to` branch in services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/attach_disk.rs) — build the refusal from the disk's state with AWS's template; the template belongs in the crate's one owner of disk state-guard refusals so attach and delete cannot word the same condition two different ways.
Traces: lightsail.disk-attach-already-attached-rejected.

Cause 2 of 3: DeleteDisk on a disk still attached to an instance refuses with the state sentence AWS uses for attach, and never names the instance holding the disk.
AWS: Smithy (com.amazonaws.lightsail#DeleteDisk) — input diskName (ResourceName, required), forceDeleteAddOns (boolean); modeled error OperationFailureException (HTTP 400). Recorded behavior: code OperationFailureException with message "You can't perform this operation because the disk is still attached to a Lightsail instance: <instanceName>." Fixed text: "You can't perform this operation because the disk is still attached to a Lightsail instance: " + <instanceName> + "."; the single variable part is the name of the instance the disk is attached to, read from the emulator's state (never the disk name, never a state word).
Emulator: status 400 and code OperationFailureException are right, but message "You can't delete this disk right now. The state of this disk is: in-use." — it reports the disk's DiskState and omits the instance entirely, so the caller is not told which instance to detach from.
Why: AWS's message is the only place the blocking instance is named; a cleanup path that detaches the named holder and retries cannot find the holder, and matching on AWS's wording fails.
Fix: the DeleteDisk handler's not-available guard (`not_deletable` in services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/delete_disk.rs) — when the disk is blocked because it is attached, refuse with AWS's attached-to-instance template built from the disk's recorded attachment, and keep the state-wording template for the non-attached non-available states.
Traces: lightsail.disk-delete-while-attached-rejected.

Cause 3 of 3: DeleteBucket without forceDelete on a non-empty bucket appends the emulator's own condition inventory and remediation advice to AWS's message.
AWS: Smithy (com.amazonaws.lightsail#DeleteBucket) — input bucketName (BucketName, required, length 3..54, pattern ^[a-z0-9][a-z0-9-]{1,52}[a-z0-9]$), forceDelete (boolean); modeled error InvalidInputException (HTTP 400), documented "Lightsail throws this exception when user input does not conform to the validation rules of an input field". Recorded behavior: code InvalidInputException with message "Bucket is not empty: <bucketName>" and nothing after it. Fixed text: "Bucket is not empty: "; the only variable part is the bucketName copied from the request.
Emulator: status 400 and code InvalidInputException are right, but the message is "Bucket is not empty: <bucketName> (it holds N object(s), and it has M access key(s)); delete what the bucket holds, or set forceDelete to delete it anyway" — AWS's headline followed by a parenthesised, pluralised inventory of the blocking conditions and a remediation clause AWS does not emit. The counts and the clause are the emulator's invention, not request- or AWS-template values.
Why: any consumer comparing the message to AWS's (assertion, log rule, console string) fails, and the appended counts read as authoritative AWS prose that real AWS never returns.
Fix: the DeleteBucket handler's force-delete guard in services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/delete_bucket.rs (the `blocked_by` assembly that wraps the headline) — serve the headline template alone; the per-condition detail may be logged, not returned in the modelled message.
Traces: lightsail.bucket-delete-nonempty-without-force-rejected.

Related open issue, NOT a duplicate: 20261029T234852Z-lightsail-client-error-bodies-carry-the-wrong-code.md covers the error bodies of PutAlarm/DeleteAlarm/CreateCloudFormationStack (wrong `code` discriminator); the three causes here are different operations and are message-text defects on refusals whose code is already correct.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (lightsail 15/25, 10 failed). All three checks reached the emulator, got the right HTTP status and the right error code, and failed on the message assertion only.

Cause 1 — lightsail.disk-attach-already-attached-rejected (mismatch):
  code: expected 'OperationFailureException', got 'OperationFailureException' (matches)
  message: expected "You can't attach this disk right now. The state of this disk is: in-use.", got 'The disk <diskName> is already attached to the instance <instanceName>. Detach the disk before attaching it to another instance.'
  Emulator source: src/ops/attach_disk.rs lines 236-241 (the `attached_to` guard composing that sentence).

Cause 2 — lightsail.disk-delete-while-attached-rejected (mismatch):
  code: expected 'OperationFailureException' (matches)
  message: expected "You can't perform this operation because the disk is still attached to a Lightsail instance: <instanceName>.", got "You can't delete this disk right now. The state of this disk is: in-use."
  Emulator source: src/ops/delete_disk.rs lines 228-239 (`not_deletable`, built from the DiskState value).

Cause 3 — lightsail.bucket-delete-nonempty-without-force-rejected (mismatch):
  code: expected 'InvalidInputException' (matches)
  message: expected 'Bucket is not empty: <bucketName>', got 'Bucket is not empty: <bucketName> (it holds 1 object, and it has 1 access key); delete what the bucket holds, or set forceDelete to delete it anyway'
  Emulator source: src/ops/delete_bucket.rs lines 218-260 (`blocked_by` inventory appended to the headline).

(Disk names, instance names, bucket names and the object/access-key counts above are request- and state-derived values; the template around them is the requirement.)

## Steps to Reproduce
Boot the emulator and point a botocore lightsail client at its data listener in the emulator's region.

Cause 1: create two instances in one AZ and wait for `running`; create_disk(diskName=D, availabilityZone=same AZ, sizeInGb=8) and wait for `available`; attach_disk(diskName=D, instanceName=A, diskPath="/dev/xvdf") and wait for `in-use`; then attach_disk(diskName=D, instanceName=B, diskPath="/dev/xvdg"). Expect HTTP 400, code OperationFailureException, message "You can't attach this disk right now. The state of this disk is: in-use.". Observed the "already attached to the instance ... Detach the disk" sentence.

Cause 2: with disk D attached to instance A, call delete_disk(diskName=D). Expect HTTP 400, code OperationFailureException, message "You can't perform this operation because the disk is still attached to a Lightsail instance: A.". Observed "You can't delete this disk right now. The state of this disk is: in-use.".

Cause 3: create_bucket; create_bucket_access_key; PutObject one object into the bucket; then delete_bucket(bucketName=B) with no forceDelete. Expect HTTP 400, code InvalidInputException, message exactly "Bucket is not empty: B". Observed that headline followed by "(it holds 1 object, and it has 1 access key); delete what the bucket holds, or set forceDelete to delete it anyway".

Re-replay: lightsail.disk-attach-already-attached-rejected, lightsail.disk-delete-while-attached-rejected, lightsail.bucket-delete-nonempty-without-force-rejected.
