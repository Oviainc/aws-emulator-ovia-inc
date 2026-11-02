Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-repository-lifecycle-and-metadata
Issue-Step: task#10
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-repository-lifecycle-and-metadata
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|BatchGetRepositories|2xx->2xx|$.repositories[*].cloneUrlHttp
Issue-Key: codecommit|BatchGetRepositories|2xx->2xx|$.repositories[*].cloneUrlSsh
Issue-Commit: 307ae70e1176afe8148743d9dbd874f3de53a94d

# CodeCommit BatchGetRepositories returns the repositories list in request order

> _Note: this issue was identified against the code base at commit `307ae70e1176afe8148743d9dbd874f3de53a94d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetRepositories

## Description
Cause 1 of 1: `BatchGetRepositories` assembles its `repositories` list strictly in the order the request's `repositoryNames` listed the names, but real CodeCommit answers the SAME request with the same two repositories in the opposite order, so entry-for-entry the emulator's list is transposed relative to AWS.

AWS: the model says nothing about ordering — `BatchGetRepositoriesOutput.repositories` is documented only as "A list of repositories returned by the batch get repositories operation." (Smithy: `smithy_query(query="doc", shape="BatchGetRepositoriesOutput", member="repositories")`; the operation carries no `@paginated` trait and no sort member, and its input is just `repositoryNames` (`RepositoryNameList`, `@required`)). So this is recorded behavior only: the recording for this step shows CodeCommit answering 200 with the two repositories in an order that is NOT the request order. The fixture's request lists the RENAMED repository before the second repository (the emulator, which echoes request order verbatim, emits the renamed one at index 0), while the recording returns the second repository at index 0 and the renamed one at index 1. Each recorded entry carries CodeCommit's regional clone-URL form, `https://git-codecommit.<region>.amazonaws.com/v1/repos/<repositoryName>` and `ssh://git-codecommit.<region>.amazonaws.com/v1/repos/<repositoryName>`, which is how the diverging index can be read off the recording at all: the path segment of each entry's clone URLs names that entry's repository. Hypothesis: CodeCommit orders this list on something internal to the service (most-recently-created first, or a descending name/id order) rather than on the request; what the recording establishes is only that the order is NOT the request's.

Scope note — the host and port of the diverging strings are explicitly NOT part of this finding. The emulator must announce the address it is actually reached on, so `127.0.0.1:<port>` in place of `git-codecommit.<region>.amazonaws.com` is an environment-local value and no requirement is claimed for it here (an unusable announced clone URL is already tracked by `20261025T040000Z-codecommit-announced-clone-url-is-served-with-an-untrusted-certificate.md`). The part that diverges beyond the environment is WHICH repository is named at each index.

Emulator: `BatchGetRepositories::handle` walks `input.repository_names` in order, skipping names it has already seen, and pushes `repository.metadata(&endpoint)` for each name the store holds; the response list is therefore exactly the de-duplicated request order. The projection itself is faithful (`Repository::metadata` derives both clone URLs from the repository's own stored `name`, so no entry carries another repository's URL) — only the sequence of entries differs. No status or error is involved: 200 expected, 200 answered.

Why: a consumer that relies on the order AWS actually serves — a contract/golden-body comparison, a snapshot diff of the same two accounts or environments, or any UI/report that renders `repositories` in the order received and expects CodeCommit's ordering — sees a different list from the emulator than from AWS for an identical request. A consumer that keys entries by `repositoryName` is unaffected, which is why this is filed LOW rather than higher.

Fix: the response-assembly loop in the operation's handler. The `repositories` list must be ordered the way CodeCommit orders it (an order derived from the repositories themselves, not from the request's name list); the de-duplication rule, the `repositoriesNotFound` list and the always-empty `errors` list are correct as they stand and should not change.

Traces: `codecommit-repository-lifecycle-and-metadata#task#10` (representative, and the cluster's only member). Code: `services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/batch_get_repositories.rs:218-229` (the request-order assembly loop), `services/service-com-amazonaws-codecommit-codecommit-20150413/src/state.rs:2243-2257` (`Repository::metadata`, which derives each entry's clone URLs from that entry's own name).

No open CodeCommit issue covers the ORDER of this list: the nearest ones are about the announced clone URL being unusable (`20261025T040000Z-codecommit-announced-clone-url-is-served-with-an-untrusted-certificate.md`) and about `repositoryMetadata.kmsKeyId` reporting an alias path (`20261102T022028Z-codecommit-repositorymetadata-kmskeyid-reports-the-alias-path-instead.md`) — neither concerns which repository appears at which index.

## Evidence
Replay of fixture `codecommit-repository-lifecycle-and-metadata`, step `task#10` (BatchGetRepositories), verdict `mismatch`, status 200 expected and 200 observed. Cluster key `service=codecommit|BatchGetRepositories|status=200->200|keys=$.repositories[0].cloneUrlHttp,$.repositories[0].cloneUrlSsh,$.repositories[1].cloneUrlHttp,$.repositories[1].cloneUrlSsh`, 1 affected step. Reported detail, verbatim:

  $.repositories[0].cloneUrlHttp: value_mismatch (expected 'https://git-codecommit.us-west-2.amazonaws.com/v1/repos/sim2real-probe-codecommit-repo2-e8526d35', got 'https://127.0.0.1:44019/v1/repos/sim2real-probe-codecommit-repo-renamed-e8526d35')
  $.repositories[0].cloneUrlSsh: value_mismatch (expected 'ssh://git-codecommit.us-west-2.amazonaws.com/v1/repos/sim2real-probe-codecommit-repo2-e8526d35', got 'ssh://127.0.0.1/v1/repos/sim2real-probe-codecommit-repo-renamed-e8526d35')
  $.repositories[1].cloneUrlHttp: value_mismatch (expected 'https://git-codecommit.us-west-2.amazonaws.com/v1/repos/sim2real-probe-codecommit-repo-renamed-e8526d35', got 'https://127.0.0.1:44019/v1/repos/sim2real-probe-codecommit-repo2-e8526d35')
  $.repositories[1].cloneUrlSsh: value_mismatch (expected 'ssh://git-codecommit.us-west-2.amazonaws.com/v1/repos/sim2real-probe-codecommit-repo-renamed-e8526d35', got 'ssh://127.0.0.1/v1/repos/sim2real-probe-codecommit-repo2-e8526d35')

Reading the repository path segment of each entry: the recording's `repositories[0]` is the repository whose name ends `-repo2-...` and its `repositories[1]` is the one ending `-repo-renamed-...`; the emulator answers the reverse at both indices. The two entries are therefore transposed, and the recorded names are run-local probe names whose literal text is not the requirement — the requirement is that the entry at a given index is the SAME repository AWS puts there.

The host/port difference inside the same four strings (`127.0.0.1:44019` and `127.0.0.1` vs `git-codecommit.us-west-2.amazonaws.com`, and the ssh form dropping the port) is environment-local: the emulator announces the address the request arrived on (`ServingEndpoint::from_request`, used by the handler at `batch_get_repositories.rs:181`). No requirement is asserted for it in this issue.

Every other member of both entries matched — `repositoryId`, `arn`, `accountId`, `repositoryDescription`, `defaultBranch`, the dates and `kmsKeyId` are all absent from the divergence key — so the projection is right and only the sequencing of the list diverges.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. `aws codecommit create-repository --repository-name demo-one`, then `aws codecommit update-repository-name --old-name demo-one --new-name demo-one-renamed`, then `aws codecommit create-repository --repository-name demo-two`.
3. Call `aws codecommit batch-get-repositories --repository-names demo-one-renamed demo-two`.
4. Read the order of `repositories` in the 200 response: the emulator returns the entries in the order the request named them (`demo-one-renamed` at index 0, `demo-two` at index 1). Real CodeCommit, as the recording shows for this step, returns the two the other way round — the later-created repository first — i.e. the list order is not the request's.
5. Re-replay fixture `codecommit-repository-lifecycle-and-metadata` step `task#10` to confirm the fix; after it, the repository named at `$.repositories[0]` and `$.repositories[1]` must match the recording (ignore the host:port of the clone URLs, which is environment-local).
