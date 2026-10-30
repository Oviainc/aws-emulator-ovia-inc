Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-instance-event-window-tag-association-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-instance-event-window-tag-association-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:DisassociateInstanceEventWindow
Issue-Commit: c42b5bef1562db44010b2579a8522ff39a3564f6

# Implement ec2 DisassociateInstanceEventWindow: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c42b5bef1562db44010b2579a8522ff39a3564f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateInstanceEventWindow

## Description
`DisassociateInstanceEventWindow` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/disassociate_instance_event_window.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DisassociateInstanceEventWindow` call, so each one unblocks only when `DisassociateInstanceEventWindow` works: `ec2-instance-event-window-tag-association-lifecycle#task#5`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/disassociate_instance_event_window.rs`. Replaying the corpus, the emulator refused the `DisassociateInstanceEventWindow` call: status: expected 200, got 501
$.DisassociateInstanceEventWindowResponse: missing (expected {'requestId': '<shaped-not-asserted>', 'instanceEventWindow': {'associationTarget': {'dedicatedHostIdSet': '', 'instanceIdSet': '', 'tagSet': ''}, 'instanceEventWindowId': 'iew-00000000000000022', 'name': 'sim2real-probe-ec2-event-window-c12ca7df', 'state': 'disassociating', 'timeRangeSet': {'item': {'endHour': '6', 'endWeekDay': 'sunday', 'startHour': '0', 'startWeekDay': 'sunday'}}}}, got None)

## Steps to Reproduce
Boot the ec2 emulator
Call DisassociateInstanceEventWindow and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
