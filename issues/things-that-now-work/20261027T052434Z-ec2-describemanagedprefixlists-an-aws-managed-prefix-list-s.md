Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__s3.managed-prefix-list-id-agrees-across-both-describe-apis
Issue-Key: ec2__lambda__s3.managed-prefix-list-id-agrees-across-both-describe-apis|run#14|AssertionError|assert m["PrefixListArn"] == f"arn:aws:ec2:{ctx.region}:aws:prefix-list/{m['PrefixListId']}", (
Issue-Commit: 6faa7ce4aec2ce039d1c72fb49683f578ec3667c

# EC2 DescribeManagedPrefixLists: an AWS-managed prefix list's PrefixListArn carries the caller's account id instead of the literal 'aws' account segment

> _Note: this issue was identified against the code base at commit `6faa7ce4aec2ce039d1c72fb49683f578ec3667c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeManagedPrefixLists

## Description
One net-new root cause in an EC2 read path that now DOES answer (no scaffold stub is involved any more).

Cause 1 of 1: DescribeManagedPrefixLists builds the PrefixListArn of a SERVICE-MANAGED prefix list from the caller's account id, so the ARN's account segment is the account rather than the literal `aws`.

AWS: Smithy (com.amazonaws.ec2#AmazonEC2) models ManagedPrefixList.PrefixListArn -> ResourceArn, xmlName `prefixListArn`, ec2QueryName `PrefixListArn`, @length min 1 max 1283, documented as "The Amazon Resource Name (ARN) for the prefix list." The same structure models OwnerId as "The ID of the owner of the prefix list", and for an AWS-managed list (the per-region service lists such as `com.amazonaws.<region>.s3`) that owner is the literal string `AWS`, not an account id — the emulator already returns OwnerId `AWS` here, so it has classified the list correctly. The model does not spell out the ARN's internal format, so for the account segment this is recorded behavior only: a service-managed prefix list's ARN is `arn:aws:ec2:<region>:aws:prefix-list/<prefix-list-id>` — fixed text `arn:aws:ec2:`, then the request's region, then the literal segment `aws`, then `:prefix-list/`, then the list's own id. Only customer-managed lists (created by CreateManagedPrefixList, OwnerId = a 12-digit account id) carry the account id in that segment.

Emulator: for the S3 service list the same response carries PrefixListArn `arn:aws:ec2:us-west-2:000000000000:prefix-list/pl-fffbb61c` — the region segment is right, the id segment is right and agrees with PrefixListId, but the account segment is the caller's account id instead of `aws`. The preceding members of the same record are correct (PrefixListId agrees with DescribePrefixLists, PrefixListName is `com.amazonaws.<region>.s3`, OwnerId is `AWS`, State is `create-complete`, AddressFamily is `IPv4`), which is what isolates the ARN composition as the single defect: the handler knows the list is AWS-owned yet still formats the ARN with the caller account.

Why: the ARN is the form a client hands to IAM and to resource-level tooling. A caller that reads PrefixListArn off DescribeManagedPrefixLists and writes it into an IAM policy condition, a tag-API lookup or a cross-account reference produces an ARN that names an account-owned resource that does not exist, so the policy never matches and the lookup never resolves; conversely a caller that branches on "does the ARN's account segment equal my account id?" to decide whether a prefix list is modifiable concludes the AWS-managed S3 list is its own and will attempt ModifyManagedPrefixList / DeleteManagedPrefixList against it. The two signals the response gives for the same fact (OwnerId `AWS` vs an account id in the ARN) also contradict each other, so there is no consistent reading.

Fix: ARN composition for the managed-prefix-list projection in the DescribeManagedPrefixLists handler / its serializer — the account segment must be derived from the list's ownership (literal `aws` when the list is service-managed, the owning account id when it is customer-managed) rather than unconditionally from the request's account. The seeded service prefix lists in the EC2 state should carry that ownership so the serializer can branch on it.

Related, but NOT a duplicate: the open issue 20261022T235108Z-ec2-describemanagedprefixlists-stub.md lists this same check id under Issue-Trace-Ids, but it is about DescribeManagedPrefixLists answering HTTP 501 NotImplemented as an unmodified scaffold. The operation now answers 200 with populated data and the replay reaches the ARN assertion, so that refusal is no longer what this check fails on; what remains is the ARN-composition defect described above. Nothing here is a refusal, so this is not a reopen of a fixed issue.

Traces: ec2__lambda__s3.managed-prefix-list-id-agrees-across-both-describe-apis.

## Evidence
Check ec2__lambda__s3.managed-prefix-list-id-agrees-across-both-describe-apis, verdict `mismatch`.

Failing assertion (check_ec2__lambda__s3_managed_prefix_list_id_agrees_across_both_describe_apis.py line 78):
  assert m["PrefixListArn"] == f"arn:aws:ec2:{ctx.region}:aws:prefix-list/{m['PrefixListId']}"

AssertionError: the ARN reads 'arn:aws:ec2:us-west-2:000000000000:prefix-list/pl-fffbb61c'; an AWS-managed list's ARN carries the literal account segment 'aws', not the caller's account id

Expected (template; region and prefix-list id come from the request/state, `aws` and the rest are fixed text):
  arn:aws:ec2:<region>:aws:prefix-list/<PrefixListId>
Actual:
  arn:aws:ec2:<region>:<caller-account-id>:prefix-list/<PrefixListId>

HTTP status 200 in both cases — this is a wrong value in a successful response, not a refusal. The assertions executed before it all passed on the same record, so PrefixListId, PrefixListName, OwnerId == 'AWS', State == 'create-complete' and AddressFamily == 'IPv4' are correct and only the ARN diverges.

## Steps to Reproduce
1. Boot the emulator and point an EC2 client at it.
2. Call ec2:DescribeManagedPrefixLists with Filters=[{"Name": "prefix-list-name", "Values": ["com.amazonaws.<region>.s3"]}] (use the client's own region).
3. Read PrefixLists[0]: note OwnerId == "AWS" (correct, the list is service-managed) and then read PrefixListArn.
4. Observe the ARN's 5th segment is the caller's account id; AWS returns the literal `aws` there, i.e. arn:aws:ec2:<region>:aws:prefix-list/<PrefixLists[0].PrefixListId>.
5. Replay ec2__lambda__s3.managed-prefix-list-id-agrees-across-both-describe-apis to confirm the fix; note the check continues past this assertion into the DescribePrefixLists projection checks (Cidrs present on the legacy view only, OwnerId/State absent from it, Cidrs absent from the managed view), which were never reached in this run.
