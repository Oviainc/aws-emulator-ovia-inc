Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.delete-file-on-tag-named-branch-blocked codecommit.git-push-non-fast-forward-refused-by-the-server codecommit.git-push-over-https-is-readable-through-the-api
Issue-Key: codecommit.delete-file-on-tag-named-branch-blocked|_run#24|RuntimeError|raise RuntimeError(
Issue-Key: codecommit.git-push-non-fast-forward-refused-by-the-server|_retry_git#27|Timeout|time.sleep(interval_s)
Issue-Key: codecommit.git-push-over-https-is-readable-through-the-api|_retry_git#28|Timeout|time.sleep(interval_s)
Issue-Commit: 144a2fce0f9e3116760062dd946ebab581523079

# CodeCommit: the announced cloneUrlHttp is served with a certificate no git client trusts, so clone and push are unreachable

> _This report is the RESIDUAL cause of
> `20261023T185908Z-codecommit-8-net-new-behavioral-root-causes-the.md` (its Cause 1 of 8).
> The other seven causes of that report — GetFolder's empty collections, the
> ParentCommitIdOutdated / ParentCommitIdRequired / NoChange / SameFileContent /
> BlobIdDoesNotExist / BranchDoesNotExist message-and-shape rules, and the HEAD
> default for an absent commitSpecifier — are fixed and that report is resolved.
> This one is filed on its own because it is not reachable from the service crate:
> see "Why this is not fixable in `services/`" below._

## Operation
GetRepository (`repositoryMetadata.cloneUrlHttp`) and the git smart-HTTP transport it announces

## Description
The git clone URL the service announces cannot be used by a standard git client: the
transport answers, but with a TLS certificate the client's trust store rejects, so every
clone and push fails before any git protocol exchange.

AWS: `GetRepository.repositoryMetadata.cloneUrlHttp` is an address the service answers git
smart-HTTP on over TLS with a publicly trusted certificate. With CodeCommit git
credentials, `git clone <cloneUrlHttp>` succeeds, `git push origin <tag>` creates a ref the
control plane then sees, and a push that is not a fast-forward is refused inside
receive-pack. CodeCommit has no API operation that creates a tag, so the
tag-vs-branch and server-side push-refusal behaviours are reachable only over that
transport.

Emulator: `git push` against the announced URL exits 128 with
`fatal: unable to access 'https://127.0.0.1:<port>/v1/repos/<repo>/': server verification
failed: certificate signer not trusted. (CAfile: /etc/ssl/certs/ca-certificates.crt CRLfile:
none)`. The host and port vary per run and are not the requirement; the requirement is that
the announced `cloneUrlHttp` be reachable by an unmodified git client. The two other checks
exhaust their clone/push poll on the same rejection and report `Check timed out after 60.0s`.

## Evidence
`codecommit.delete-file-on-tag-named-branch-blocked` [error]: `RuntimeError: command ['git',
'push', 'origin', 'main'] failed (rc=128)` with the `certificate signer not trusted` stderr
above — the grounding evidence. `codecommit.git-push-non-fast-forward-refused-by-the-server`
and `codecommit.git-push-over-https-is-readable-through-the-api` [mismatch]: `Check timed
out after 60.0s` with no git exchange completed.

## Why this is not fixable in `services/`
The transport itself is implemented and reads the same object store the API reads:
`services/service-com-amazonaws-codecommit-codecommit-20150413/src/service/git_http.rs`
serves `info/refs`, `git-upload-pack` and `git-receive-pack` under the
`/v1/repos/<repositoryName>` path the announced URL names. What the client rejects is the
CERTIFICATE, and every listener the core offers a service terminates TLS itself with a
self-signed certificate:

* `vendor/server/src/listeners.rs:239` — `Listeners::tcp`, "TLS-only", with the acceptor at
  `:301` noting that "a plaintext/HTTP preface also fails the handshake here and never
  reaches the handler";
* `vendor/server/src/listeners.rs:334` — `Listeners::http`, likewise TLS, from the
  self-signed material generated at `:211`;
* a service may not bind a socket of its own — the same core limitation this workspace
  already records for a native data plane in
  `services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:5335-5349`.

So no address the crate could announce is reachable by an unmodified git client:
`https://` fails signer verification, `http://` has no plaintext listener to name, and
`ssh://` has no listener at all. Closing this needs a core listener seam that either hands
a connection to the service WITHOUT terminating TLS (so the service can serve plaintext
git smart-HTTP on a loopback port and announce it), or terminates TLS with a certificate
the client's trust store accepts.

## Steps to Reproduce
1. `meta = cc.create_repository(repositoryName=R)['repositoryMetadata']`.
2. Run `git clone <meta['cloneUrlHttp']>` with an unmodified git client -> expect the clone
   to complete; observe git exiting 128 with `server verification failed: certificate signer
   not trusted`.
3. Re-replay the three check ids in `Issue-Trace-Ids`.
