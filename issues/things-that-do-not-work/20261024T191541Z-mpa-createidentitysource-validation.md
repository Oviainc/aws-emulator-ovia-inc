Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 3a66b075702e523a87f5e33a7353c602a74f8e6d

# CreateIdentitySource accepts reserved aws: tag keys and a Region value that is not an AWS Region

_Filed: 2026-10-24T19:15:41Z_
_Operation: CreateIdentitySource | Class: validation_

> _Note: this issue was identified against the code base at commit `3a66b075702e523a87f5e33a7353c602a74f8e6d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: (1) tag keys in the reserved `aws:` namespace are refused with ValidationException (400) — AWS reserves the `aws:` prefix for system tags on every taggable resource and no public create/tag API may store one; (2) `IdentitySourceParameters.IamIdentityCenter.Region` names the Region the IAM Identity Center instance lives in, so a value that is not a Region (e.g. `not a region!!` or `mars-central-9`) is refused with ValidationException (400).

Actual: src/ops/create_identity_source.rs validates only the tag COUNT (against MAX_TAGS_PER_RESOURCE) and then copies the map verbatim into `IdentitySource::creating(...)`, so `{"aws:cloudformation:stack-name": "x"}` is stored and later reported by ListTagsForResource as an ordinary tag. `Region` is a plain string with only `@length(max: 1000)` in the model, so the framework boundary accepts any non-empty value and the handler stores it unchecked; ListIdentitySources then reports the bogus Region back for the source's IamIdentityCenter parameters.

## Evidence

Spec: `Tags` key targets `TagKey` (length 1..128, no pattern), so the reserved-namespace rule is a service rule the model cannot express — it is well-established AWS tagging behavior (the same rule is already filed against this crate's TagResource and CreateApprovalTeam paths, which share the hole; CreateIdentitySource is a third, unfiled path). `IamIdentityCenter$Region` is documented as 'Amazon Web Services Region where the IAM Identity Center instance is located' with only @required + @length(max:1000) (smithy_query shape IamIdentityCenter). Handler: src/ops/create_identity_source.rs lines 277-316 — tag count check only, then `input.tags` cloned into the stored record; `parameters.region.clone()` stored with no check. Tests: tests/all/identity_sources.rs only exercises valid Regions (us-west-2, us-east-1, us-gov-west-1, eu-west-1) and tests/all/management_account_gate_order.rs only probes the modelled length bound and absence of Region, so neither the `aws:` tag prefix nor a non-Region Region is asserted anywhere.

## Steps to Reproduce

1. Record the caller as the organization management account so CreateIdentitySource is reachable (see the management-account gate issue).
2. POST /identity-sources with IdentitySourceParameters={IamIdentityCenter:{InstanceArn:'arn:aws:sso:::instance/ssoins-1234567890abcdef', Region:'us-west-2'}} and Tags={'aws:cloudformation:stack-name':'probe'}.
3. Observe HTTP 200 and then GET /tags/{IdentitySourceArn} reporting the `aws:`-prefixed key; expected 400 ValidationException and no resource created.
4. POST /identity-sources again with the same InstanceArn form but Region='not a region!!'.
5. Observe HTTP 200 and POST /identity-sources/?List reporting Region='not a region!!' for that source; expected 400 ValidationException.
