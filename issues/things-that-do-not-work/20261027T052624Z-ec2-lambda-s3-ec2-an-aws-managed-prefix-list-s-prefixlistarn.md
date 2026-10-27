Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__s3.managed-prefix-list-id-agrees-across-both-describe-apis
Issue-Key: ec2__lambda__s3.managed-prefix-list-id-agrees-across-both-describe-apis|run#14|AssertionError|assert m["PrefixListArn"] == f"arn:aws:ec2:{ctx.region}:aws:prefix-list/{m['PrefixListId']}", (
Issue-Commit: fbdf9d3baa643423c5a7eac120dec57cfd2abcbc

# ec2: an AWS-managed prefix list's PrefixListArn carries the caller's account id instead of the literal 'aws' account segment

> _Note: this issue was identified against the code base at commit `fbdf9d3baa643423c5a7eac120dec57cfd2abcbc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeManagedPrefixLists

## Description
Cause 1 of 1 — DescribeManagedPrefixLists reports a service-managed prefix list with a customer-account ARN.
AWS: Smithy (ec2) ManagedPrefixList.PrefixListArn is documented as 'The Amazon Resource Name (ARN) for the prefix list' (target ResourceArn). For an AWS-managed prefix list — the ones named `com.amazonaws.<region>.<service>`, which report OwnerId as the literal string `AWS` — the ARN's account segment is the literal `aws`, not an account id: `arn:aws:ec2:<region>:aws:prefix-list/<prefix-list-id>`. The region and the prefix-list id come from the list itself; the `aws` segment is fixed and is what marks the list as service-managed rather than customer-managed. Recorded behavior only for the literal segment (the model documents the member but not its account rule); the same response's OwnerId is already correctly `AWS` in the emulator, so the two fields contradict each other.
Emulator: the ARN is minted with the caller's account id — `arn:aws:ec2:<region>:000000000000:prefix-list/<prefix-list-id>` — while OwnerId on the same object reads `AWS`.
Why: the ARN is what a client puts in an IAM policy resource, a security-group or route-table reference, or a cross-account grant. An ARN naming the caller's account for a list the caller does not own resolves to nothing in real AWS, and policies generated from the emulator's answer are silently wrong. It also makes a managed list indistinguishable from a customer-created one by ARN shape.
Fix: the ARN construction for managed prefix lists in the DescribeManagedPrefixLists handler / the prefix-list state that mints the ARN — the account segment must be the literal `aws` whenever the list is service-managed (OwnerId `AWS`).
Traces: ec2__lambda__s3.managed-prefix-list-id-agrees-across-both-describe-apis.

## Evidence
AssertionError: the ARN reads 'arn:aws:ec2:us-west-2:000000000000:prefix-list/pl-fffbb61c'; an AWS-managed list's ARN carries the literal account segment 'aws', not the caller's account id. The same ManagedPrefixList entry correctly reports OwnerId == 'AWS', State == 'create-complete' and AddressFamily == 'IPv4', so only the ARN's account segment diverges. (Account id and prefix-list id shown are this run's values; the requirement is the literal 'aws' segment with the list's own region and id.)

## Steps to Reproduce
1. Call DescribeManagedPrefixLists with Filters=[{'Name': 'prefix-list-name', 'Values': ['com.amazonaws.<region>.s3']}].
2. Read the single returned PrefixLists entry's OwnerId and PrefixListArn.
3. OwnerId is 'AWS' but PrefixListArn's account segment is the caller's account id instead of the literal 'aws'.
