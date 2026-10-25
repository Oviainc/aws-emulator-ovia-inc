Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: memorydb.describe-users-pages-cover-every-user
Issue-Shape-Id: com.amazonaws.memorydb#AmazonMemoryDB
Issue-Key: memorydb|DescribeUsers|InvalidParameterValueException
Issue-Commit: 68f9532db1e883ddd5ef2ff638d6034db2e56113

# DescribeUsers/DescribeACLs refuse MaxResults in 1..19: the 20..100 page-size bound was applied to the user/ACL describes, whose AWS bound is 1..50

> _Note: this issue was identified against the code base at commit `68f9532db1e883ddd5ef2ff638d6034db2e56113`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeUsers

## Description
REOPENS 20261023T161602Z-memorydb-maxresults-bounds-unenforced-and-validation-text-diverges.md: its fix left the call refused.

One cause.

Cause 1 of 1: DescribeUsers (and, on the same validation path, DescribeACLs) reject a MaxResults value that AWS accepts, because the MemoryDB 20..100 page-size bound that applies to the cluster/parameter/event describes was applied to the user and ACL describes as well.

AWS: DescribeUsers and DescribeACLs are paginated with pageSize=MaxResults (Smithy: DescribeUsers paginated {inputToken: NextToken, outputToken: NextToken, items: Users, pageSize: MaxResults}; DescribeACLs paginated with items: ACLs; MaxResults targets IntegerOptional with NO @range on either operation), so the accepted page-size range is recorded behavior only. The recording shows MaxResults=1 ACCEPTED (one item per page, NextToken advancing until exhausted) and only values outside 1..50 refused, with HTTP 400 / error code InvalidParameterValueException and message of the form 'Invalid value for max results. Must be between 1 and 50' (the recorded text; the rule is the 1..50 bound). Note that the bound differs per operation family in MemoryDB: the cluster/parameter/event/engine-version describes use 20..100, the user/ACL describes use 1..50.

Emulator: DescribeUsers(MaxResults=1) is refused with HTTP 400 InvalidParameterValueException and message 'Invalid value for max records. Must be between 20 and 100' — i.e. the 20..100 bound (and the 'max records' wording) from the other describe family. Every value in 1..19 that AWS accepts is refused, and values in 51..100 that AWS refuses are presumably accepted. Hypothesis: the earlier fix introduced one shared MaxResults validator and wired it into every paginated Describe* entry point, including DescribeUsers and DescribeACLs.

Why: a client cannot page users or ACLs at all with the small page sizes AWS permits — the first call of any MaxResults<20 paging loop fails hard, so pagination coverage, exactly-once traversal and NextToken handling are unreachable against the emulator. Conversely a client passing 51..100 gets a page where AWS would fault.

Fix: the MemoryDB MaxResults validation layer — give the user/ACL describes (DescribeUsers, DescribeACLs) their own 1..50 bound and 'max results' message instead of the shared 20..100 'max records' bound, rather than applying one bound to every paginated Describe*.

Traces: memorydb.describe-users-pages-cover-every-user (the check's paging loop fails on its first DescribeUsers call with MaxResults=1; the same loop then exercises DescribeACLs with MaxResults=1 and the out-of-range cases 0 and 51, which were never reached).

## Evidence
Check memorydb.describe-users-pages-cover-every-user: after creating three users and three ACLs, the paging loop calls DescribeUsers(MaxResults=1) and botocore raises InvalidParameterValueException: 'An error occurred (InvalidParameterValueException) when calling the DescribeUsers operation: Invalid value for max records. Must be between 20 and 100'. Expected: HTTP 200 with at most one user plus a NextToken, and a refusal only for MaxResults outside 1..50 (code InvalidParameterValueException, message form 'Invalid value for max results. Must be between 1 and 50'). Actual: HTTP 400 for the in-range value 1, quoting a 20..100 bound. Because the call is refused, the later assertions of the same check (DescribeACLs paging, MaxResults=0 and MaxResults=51 refusals, the user-name filter, and the unknown-filter-name message 'Invalid filter name UserName.') were never evaluated.

## Steps to Reproduce
1. Create a few MemoryDB users (CreateUser with AccessString 'on ~* +@all' and a password AuthenticationMode) and ACLs (CreateACL).
2. Call DescribeUsers(MaxResults=1). AWS returns 200 with one user and a NextToken; the emulator returns HTTP 400 InvalidParameterValueException 'Invalid value for max records. Must be between 20 and 100'.
3. Repeat with DescribeACLs(MaxResults=1) — same validation path.
4. Call DescribeUsers(MaxResults=0) and DescribeUsers(MaxResults=51): AWS refuses both with InvalidParameterValueException and the 1..50 bound; DescribeUsers(MaxResults=50) must be accepted.
Replay: behavioral check memorydb.describe-users-pages-cover-every-user.
