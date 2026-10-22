Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.git-push-over-https-is-readable-through-the-api codecommit.git-push-non-fast-forward-refused-by-the-server codecommit.delete-file-on-tag-named-branch-blocked
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit.git-push-over-https-is-readable-through-the-api|_retry_git#28|Timeout|time.sleep(interval_s)
Issue-Key: codecommit.git-push-non-fast-forward-refused-by-the-server|_retry_git#27|Timeout|time.sleep(interval_s)
Issue-Key: codecommit.delete-file-on-tag-named-branch-blocked|_run#24|RuntimeError|raise RuntimeError(
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeCommit serves no git smart-HTTP transport, and the cloneUrlHttp it announces is not an address it answers on

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRepository

## Description
Cause 1 of 1: the git transport behind a CodeCommit repository does not exist in the emulator, and the clone URL the API hands out points somewhere the emulator does not serve.

AWS: recorded behavior only — git smart-HTTP is not part of the Smithy model. RepositoryMetadata.cloneUrlHttp (returned by CreateRepository/GetRepository) is the repository's HTTPS git endpoint, and with IAM service-specific git credentials (iam.CreateServiceSpecificCredential for codecommit.amazonaws.com) `git clone`, `git push` and `git push refs/tags/...` against that URL complete: the pushed commits and tags are then visible through the control-plane API (GetFile/GetBranch), and a non-fast-forward push is refused by the server with the losing ref left unmoved (the loser's file does not resolve at the winner's head).

Emulator: no git endpoint answers. Two checks that take the returned cloneUrlHttp and route it to the emulator's own endpoint (ctx.endpoint_route) never finish — the suite reports 'Check timed out after 60.0s' for both, with git blocked on the clone/push. A third check used the announced cloneUrlHttp verbatim and git reported "fatal: unable to access 'https://git-codecommit.us-west-2.amazonaws.com/v1/repos/<repo>/': The requested URL returned error: 403" — i.e. the response announced a host the emulator is not reached on, so the client was sent off the emulator entirely. Hypothesis: the codecommit crate implements only the JSON control plane, with no /v1/repos/<name>/info/refs, git-upload-pack or git-receive-pack handler, and cloneUrlHttp is a formatted constant rather than a value derived from the serving endpoint.

Why: every git-native workflow against the emulator is impossible — clone, push, tag creation (there is no control-plane API that creates a git tag) and the server-side non-fast-forward refusal that protects concurrent pushers. A client that reads cloneUrlHttp and hands it to git either hangs or is redirected to real AWS, where it fails authentication; and because tags cannot be created, the API behaviors that only a tag can exercise (a branchName that names a tag) cannot be reached at all.

Fix: a git smart-HTTP transport in the codecommit crate (services/service-com-amazonaws-codecommit-codecommit-20150413) bound to the repository paths, plus the cloneUrlHttp/cloneUrlSsh members of RepositoryMetadata (src/ops/create_repository.rs and src/ops/get_repository.rs) being derived from the endpoint the emulator actually serves.

Traces: codecommit.git-push-over-https-is-readable-through-the-api, codecommit.git-push-non-fast-forward-refused-by-the-server, codecommit.delete-file-on-tag-named-branch-blocked.

## Evidence
Expected: `git clone`/`git push` against repositoryMetadata.cloneUrlHttp succeed (exit 0) and the pushed refs are readable via GetFile/GetBranch; a non-fast-forward push exits non-zero and the rejected ref is unchanged.
Actual:
- codecommit.git-push-over-https-is-readable-through-the-api: 'Check timed out after 60.0s' (git clone of the routed cloneUrlHttp never returns).
- codecommit.git-push-non-fast-forward-refused-by-the-server: 'Check timed out after 60.0s'.
- codecommit.delete-file-on-tag-named-branch-blocked: RuntimeError: command ['git','push','origin','main'] failed (rc=128): stderr=fatal: unable to access 'https://git-codecommit.us-west-2.amazonaws.com/v1/repos/sim2real-behav-repo-a6deba094841/': The requested URL returned error: 403 — the URL came straight from the emulator's CreateRepository response member cloneUrlHttp.
Host names and repository names vary per run; the requirement is that the announced clone URL is served by the emulator and speaks git smart-HTTP.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. meta = cc.create_repository(repositoryName=R)['repositoryMetadata']; note meta['cloneUrlHttp'].
3. Observe that the URL names git-codecommit.<region>.amazonaws.com rather than the endpoint the emulator is reached on.
4. git init; commit a file; git remote add origin <cloneUrlHttp routed to the emulator endpoint>; git push origin main -> expect exit 0; observe the command never completes (the check's 60s deadline fires).
5. Use the URL verbatim instead -> observe git exits 128 with 'The requested URL returned error: 403' from the real AWS host.
6. Re-replay the check ids in trace_ids.
