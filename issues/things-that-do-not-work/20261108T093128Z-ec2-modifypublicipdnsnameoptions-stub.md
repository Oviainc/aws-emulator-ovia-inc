Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-modify-public-ip-dns-name-options
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|ModifyPublicIpDnsNameOptions|NotImplemented
Issue-Commit: d8469fdbee4aefb676d6a1b6581464fb2b4efb05

# Implement ec2 ModifyPublicIpDnsNameOptions: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d8469fdbee4aefb676d6a1b6581464fb2b4efb05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyPublicIpDnsNameOptions

## Description
`ModifyPublicIpDnsNameOptions` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_public_ip_dns_name_options.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ModifyPublicIpDnsNameOptions` call, so each one unblocks only when `ModifyPublicIpDnsNameOptions` works: `ec2.op-contract-modify-public-ip-dns-name-options`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_public_ip_dns_name_options.rs`. Replaying the corpus, the emulator refused the `ModifyPublicIpDnsNameOptions` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ModifyPublicIpDnsNameOptions operation: operation ModifyPublicIpDnsNameOptions is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ec2 emulator
Call ModifyPublicIpDnsNameOptions and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
