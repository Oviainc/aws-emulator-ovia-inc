Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-get-managed-prefix-list-entries
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-get-managed-prefix-list-entries ec2-managed-prefix-list-entries-and-associations
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:GetManagedPrefixListEntries
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Implement ec2 GetManagedPrefixListEntries: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetManagedPrefixListEntries

## Description
`GetManagedPrefixListEntries` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/get_managed_prefix_list_entries.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

17 recorded unit(s) fail on their `GetManagedPrefixListEntries` call, so each one unblocks only when `GetManagedPrefixListEntries` works: `ec2-catalog-get-managed-prefix-list-entries#task#1`, `ec2-catalog-get-managed-prefix-list-entries#task#2`, `ec2-catalog-get-managed-prefix-list-entries#task#3`, `ec2-catalog-get-managed-prefix-list-entries#task#4`, `ec2-catalog-get-managed-prefix-list-entries#task#5`, `ec2-catalog-get-managed-prefix-list-entries#task#6`, `ec2-catalog-get-managed-prefix-list-entries#task#7`, `ec2-catalog-get-managed-prefix-list-entries#task#8`, `ec2-catalog-get-managed-prefix-list-entries#task#9`, `ec2-catalog-get-managed-prefix-list-entries#task#10`, `ec2-catalog-get-managed-prefix-list-entries#task#11`, `ec2-catalog-get-managed-prefix-list-entries#task#12`, `ec2-catalog-get-managed-prefix-list-entries#task#13`, `ec2-catalog-get-managed-prefix-list-entries#task#14`, `ec2-catalog-get-managed-prefix-list-entries#task#15`, `ec2-catalog-get-managed-prefix-list-entries#task#16`, `ec2-managed-prefix-list-entries-and-associations#task#4`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/get_managed_prefix_list_entries.rs`. Replaying the corpus, the emulator refused the `GetManagedPrefixListEntries` call: status: expected 200, got 501
$.GetManagedPrefixListEntriesResponse: missing (expected {'requestId': '<shaped-not-asserted>', 'entrySet': {'item': [{'cidr': '107.23.255.0/26'}, {'cidr': '15.177.0.0/18'}, {'cidr': '176.34.159.192/26'}, {'cidr': '177.71.207.128/26'}, {'cidr': '54.183.255.128/26'}, {'cidr': '54.228.16.0/26'}, {'cidr': '54.232.40.64/26'}, {'cidr': '54.241.32.64/26'}, {'cidr': '54.243.31.192/26'}, {'cidr': '54.244.52.192/26'}, {'cidr': '54.245.168.0/26'}, {'cidr': '54.248.220.0/26'}, {'cidr': '54.250.253.192/26'}, {'cidr': '54.251.31.128/26'}, {'cidr': '54.252.254.192/26'}, {'cidr': '54.252.79.128/26'}, {'cidr': '54.255.254.192/26'}]}}, got None)
$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')

## Steps to Reproduce
Boot the ec2 emulator
Call GetManagedPrefixListEntries and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
