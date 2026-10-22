Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.ds-data-is-a-distinct-validated-surface
Issue-Key: ds.ds-data-is-a-distinct-validated-surface|run#7|AssertionError|assert not_found == "AccessDeniedException", (
Issue-Commit: 5e32f6bad4f368147eb90818a03953ca666b028c

# ds/ds-data cross-plane contract broken: ds-data DescribeUser discloses an absent directory as ResourceNotFoundException instead of the uniform AccessDeniedException 403 / IAM_AUTH refusal

> _Note: this issue was identified against the code base at commit `5e32f6bad4f368147eb90818a03953ca666b028c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeUser

## Description
Cause 1 of 1.
AWS: the two Directory Service planes answer the SAME well-formed but nonexistent DirectoryId differently on purpose, and that difference is the contract. The ds control plane discloses non-existence (DescribeDirectoryDataAccess -> DirectoryDoesNotExistException, HTTP 400, message naming the directory, no Reason member — the Reason discriminator exists only in the ds-data model). The ds-data data plane never confirms or denies the directory's existence: every one of its seventeen operations answers that id with AccessDeniedException, HTTP 403, modeled Reason IAM_AUTH, and a message that names the CALLER and withholds the resource. Smithy (com-amazonaws-directoryservicedata): AccessDeniedException @error client, httpError 403, Reason -> AccessDeniedReason [IAM_AUTH, DIRECTORY_AUTH, DATA_DISABLED], and its doc states the exception "can also occur when the DirectoryId doesn't exist"; DescribeUser also declares ResourceNotFoundException (HTTP 404), which the recordings (us-west-2, 2026-09-11) show is UNREACHABLE at the directory level — it is reserved for an object missing inside a directory the caller can reach.
Emulator: ds-data DescribeUser(DirectoryId=d-ff7c2c4b84) answered ResourceNotFoundException — the model-driven answer — so the data plane discloses exactly what AWS's data plane refuses to disclose, and the ds/ds-data contrast this check exists to assert collapses. The handler resolves the DirectoryId directly out of local state and falls back to not-found, with no authorization gate ahead of it, so AccessDeniedException is never constructed for an unreachable directory.
Why: a client that keys on AccessDeniedException + Reason=IAM_AUTH to decide "this directory is not reachable by me / use the control plane to find out why" never takes that branch against the emulator, and a client that keys on ResourceNotFoundException takes a branch AWS never serves. The information-hiding property is also inverted: the emulator's data plane publishes the directory's non-existence.
Fix: a shared directory-authorization gate in the ds-data crate, applied ahead of every operation handler, so an unreachable DirectoryId yields AccessDeniedException/403/IAM_AUTH and ResourceNotFoundException is only reachable for objects inside a reachable directory. This is the same emulator component as the ds-data-scoped issue filed alongside this one (title begins "ds-data: no directory-authorization gate …"); it is filed separately because this failing check's service token is `ds` and an issue carries one service.
Traces: ds.ds-data-is-a-distinct-validated-surface (the ds-grouped check that asserts the ds/ds-data relationship, including the ds-side DirectoryDoesNotExistException contrast). Related handler: services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/ops/describe_user.rs:187-198.

## Evidence
Expected (AWS): dsd.describe_user(DirectoryId='d-ff7c2c4b84', SAMAccountName=<any>) -> AccessDeniedException, HTTP 403, Reason 'IAM_AUTH', message "User: <caller principal ARN> is not authorized to access this resource"; and ds.describe_directory_data_access(DirectoryId='d-ff7c2c4b84') -> DirectoryDoesNotExistException, HTTP 400, no Reason.
Actual (emulator): AssertionError from ds.ds-data-is-a-distinct-validated-surface — "ds-data DescribeUser(DirectoryId=d-ff7c2c4b84) must raise AccessDeniedException — ds-data declines to confirm or deny that a directory exists, so it never surfaces the ResourceNotFoundException its own shape declares for an absent DirectoryId — got ResourceNotFoundException".
Code: services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/ops/describe_user.rs lines 187-198 return `crate::state::user_not_found(...)` whenever `store.directories.get(&input.directory_id)` misses; the AccessDeniedException variant declared at line 118 is never constructed.

## Steps to Reproduce
1. Boot the emulator; create botocore clients for 'ds' and 'ds-data' against it with credentials for an account holding no directory with the id below.
2. Pick an id matching ^d-[0-9a-f]{10}$ that ds.describe_directories() does not list, e.g. 'd-ff7c2c4b84'.
3. dsd.describe_user(DirectoryId='d-ff7c2c4b84', SAMAccountName='sim2real-behav-u') -> emulator raises ResourceNotFoundException; AWS raises AccessDeniedException (403, Reason IAM_AUTH, message naming the caller).
4. ds.describe_directory_data_access(DirectoryId='d-ff7c2c4b84') -> confirm the control plane DOES disclose non-existence (DirectoryDoesNotExistException, 400), i.e. the two planes must not answer the same id with the same code.
5. Re-replay check ds.ds-data-is-a-distinct-validated-surface.
