Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f3bbb57dafae938115053b9b00b150b1f9a288bb

# A spent clientToken is replayed without comparing the request, silently dropping the new call's parameters

_Filed: 2026-10-25T14:36:02Z_
_Operation: CreateVirtualCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f3bbb57dafae938115053b9b00b150b1f9a288bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateVirtualCluster` answers any second call carrying an already-spent `clientToken` with the cluster the FIRST call minted, whatever the second request asked for: a different `name`, `containerProvider`, `tags`, `securityConfigurationId`, `sessionEnabled` or `schedulerConfiguration` is accepted with a 200 and silently discarded, and the caller gets back an `id`/`name`/`arn` describing a resource it did not ask for. This crate states the opposite rule for the same `@idempotencyToken` trait in its shared state module and implements it in `CreateJobTemplate`, which fingerprints the whole request and refuses a token replayed with different parameters. `CreateSecurityConfiguration`, `CreateManagedEndpoint` and `StartJobRun` carry the identical blind-replay code, so the one rule is applied by exactly one of the five creates — fix it once (the fingerprint helper already exists) rather than in five places.

## Evidence

The rule, stated in this crate's shared state module, src/state.rs:793-800: "`clientToken` is `@idempotencyToken`, which means \"the same token names the same request\": a second call carrying a token already spent is a replay only when it really is the same request. This digest is how that is decided, so it has to cover every member that ends up on the stored record … and nothing that does not."

The handler that implements it, src/ops/create_job_template.rs:181-204:
```
        // … The same token carrying a
        // DIFFERENT request is not a replay — answering it with the first template would
        // silently drop the parameters this call asked for — so it is refused.
        …
            if existing.request_fingerprint != fingerprint {
                return Err(CreateJobTemplateErrorModeled::ValidationException(…
                    "clientToken {} was already used to create job template {}, with different parameters" …
```

The handler that skips it, src/ops/create_virtual_cluster.rs:204-220:
```
        // `clientToken` is `@idempotencyToken`: the same token replayed names the
        // cluster the first call minted, so a retried create never doubles the resource.
        …
        if let Some(existing) = replayed.and_then(|id| state.virtual_cluster(&region, &id)) {
            return Ok(CreateVirtualClusterOutput {
                id: Some(existing.id.clone()),
                name: Some(existing.name.clone()),
                arn: Some(existing.arn.clone()),
            });
        }
```
No fingerprint is stored for a cluster at all: src/state.rs:501 `pub client_token: String,` is the only token state on `VirtualClusterRecord`, against src/state.rs:845-848 `/// [`job_template_fingerprint`] of the request that minted this template, so a later call carrying the same `clientToken` is answered as a replay only when it is the same request.` on `JobTemplateRecord`.

The same blind replay: src/ops/create_security_configuration.rs:171-184, src/ops/create_managed_endpoint.rs:207-221, src/ops/start_job_run.rs:263-277.

Coverage: tests/all/job_template_idempotency.rs exercises the fingerprint rule for templates; tests/all/virtual_cluster_references.rs:375-390 only replays an IDENTICAL CreateVirtualCluster body, so no test pins the divergent-parameters case for clusters.

## Steps to Reproduce

1. POST /virtualclusters with {"name":"first","clientToken":"tok-1","containerProvider":{"id":"eks-a","type":"EKS"},"tags":{"team":"a"}} → 200, note the id.
2. POST /virtualclusters again with the SAME clientToken "tok-1" but {"name":"second","containerProvider":{"id":"eks-b","type":"EKS"},"tags":{"team":"b"}} → actual: 200 reporting the FIRST cluster's id/name/arn; the second request's name, provider and tags are dropped with no error.
3. GET /virtualclusters/{id} → the cluster still reports name "first", provider eks-a and tags {"team":"a"}.
4. Compare: POST /jobtemplates twice with one clientToken and different `jobTemplateData` → the second call is refused with ValidationException "… was already used to create job template …, with different parameters" (create_job_template.rs:194-204).
5. The same divergence holds for POST /securityconfigurations, POST /virtualclusters/{id}/endpoints and POST /virtualclusters/{id}/jobruns.
