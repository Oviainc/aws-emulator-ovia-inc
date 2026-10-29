Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__opensearch.xsvc-opensearch-3-cmk-domain-echoes-key-grants-and-serves-search
Issue-Key: kms__opensearch.xsvc-opensearch-3-cmk-domain-echoes-key-grants-and-serves-search|run#22|Timeout|time.sleep(2)
Issue-Commit: 9e55f581c86f2cc2a9694ee1b8e2f61584dc3f55

# kms__opensearch: CreateDomain with a customer-managed CMK takes no KMS grant for es.<region>.amazonaws.com, so the CMK-domain check hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `9e55f581c86f2cc2a9694ee1b8e2f61584dc3f55`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
ONE root cause, grounded in one failing behavioral check that the replay report marked neither ALREADY FILED nor OWNED.

Related open issues named for context only (neither carries this check's replay key as replayed here, so neither covers it): 20261024T221823Z-kms-opensearch-opensearch-createdomain-with-a-customer-managed-cmk-creates.md describes the same cause against commit 564feb85 with an Issue-Key frame `run#21|Timeout|time.sleep(10)` that no longer matches this check's source (the grant poll now sleeps 2 s and the timeout frame has moved), and 20261024T055421Z-kms-opensearch-createdomain-does-not-validate-encryptionatrestoptions-kmskeyid-disabled-and.md covers CreateDomain not VALIDATING KmsKeyId (disabled / cross-Region CMKs accepted), which is a different observable. The cause below is still live at this commit: even for a valid, enabled, same-Region CMK, CreateDomain never touches KMS.

Cause 1 of 1 — CreateDomain with EncryptionAtRestOptions={Enabled:true, KmsKeyId:<customer CMK>} creates no grant on that key.
AWS: creating a domain encrypted at rest with a customer-managed CMK causes OpenSearch to take a grant on that key, observable through kms ListGrants on the key. Smithy (kms#GrantListEntry): the entry carries GranteePrincipal and RetiringPrincipal (PrincipalIdType — "if the grantee principal is an Amazon Web Services service, the GranteePrincipal field contains an Amazon Web Services service principal"), IssuingAccount ("The Amazon Web Services account under which the grant was issued"), Operations ("The list of operations permitted by the grant") and Constraints; Smithy (kms#GrantConstraints): Constraints.EncryptionContextSubset is "a list of key-value pairs that must be included in the encryption context" of the cryptographic operations the grant allows. Smithy (opensearch#CreateDomain) takes EncryptionAtRestOptions{Enabled, KmsKeyId} and answers DomainStatus. Recorded behavior: the grant that appears has GranteePrincipal and RetiringPrincipal equal to the regional service principal es.<region>.amazonaws.com, IssuingAccount arn:aws:iam::<account>:root, Operations including at least Decrypt and GenerateDataKey, and Constraints.EncryptionContextSubset.domainARN equal to the new domain's ARN arn:aws:es:<region>:<account>:domain/<domain>. In those values only the region, account id and domain name come from the request and the caller's identity; the principal form, the ARN template, the operation set and the domainARN context key are the contract.
Emulator: ListGrants on the CMK stays empty forever. services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/create_domain.rs validates engine version, instance type, EBS size, master user password, warm/cold storage, log publishing, off-peak window, Cognito options, name uniqueness, domain quota and tag quota, then stores `encryption_at_rest_options: input.encryption_at_rest_options` verbatim on the Domain record (around line 389) — there is no KMS interaction anywhere in the operation and the handler holds no reference to KMS state. The check therefore spins in its ListGrants poll (600 s budget) and is killed by the suite's 60 s cap: 'Error: Check timed out after 60.0s'. Its earlier assertions (CreateKey KeyManager=='CUSTOMER', the CreateDomain and DescribeDomain echoes of EncryptionAtRestOptions) pass before the loop, and its later legs (DescribeDomain KmsKeyId after activation, and the signed _search round trip over the domain endpoint) are never reached, so they stay unverified rather than known-good.
Why: the grant is the only auditable, revocable record binding an encrypted domain to its key. Without it a client cannot see which keys its domains depend on, cannot rehearse revoking a key from a domain, and gets the opposite answer from the emulator for any least-privilege or key-inventory review: the domain reports encryption at rest while nothing in KMS records the dependency. The same class of gap is already filed for ECR in a different service (20261024T182018Z-ecr-kms-ecr-createrepository-with-encryptiontype-kms-creates-no-kms.md).
Fix: the CreateDomain handler in services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/create_domain.rs — when EncryptionAtRestOptions.Enabled is true against a customer-managed CMK, create the domain-scoped grant in KMS state (GranteePrincipal and RetiringPrincipal es.<region>.amazonaws.com, IssuingAccount arn:aws:iam::<account>:root, Operations covering Decrypt and GenerateDataKey, Constraints.EncryptionContextSubset {domainARN: <the new domain's ARN>}) so kms ListGrants reports it, and retire that grant in DeleteDomain.
Traces: kms__opensearch.xsvc-opensearch-3-cmk-domain-echoes-key-grants-and-serves-search.

## Evidence
Check kms__opensearch.xsvc-opensearch-3-cmk-domain-echoes-key-grants-and-serves-search, verdict [mismatch]: 'Error: Check timed out after 60.0s'.

The assertions before the blocking loop pass (CreateKey KeyManager=='CUSTOMER'; CreateDomain echoing EncryptionAtRestOptions=={'Enabled': True, 'KmsKeyId': <key arn>}; DescribeDomain echoing the same), so the cap is hit inside the grant poll:

    service_principal = f"es.{ctx.region}.amazonaws.com"
    deadline = time.monotonic() + 600
    while time.monotonic() < deadline:
        grants = _all_grants(kms, key["KeyId"])          # paginated kms ListGrants
        ours = [g for g in grants
                if g.get("GranteePrincipal") == service_principal
                and ((g.get("Constraints") or {}).get("EncryptionContextSubset") or {}).get("domainARN") == domain_arn]
        if ours: break
        time.sleep(2)
    assert ours, "no KMS grant to 'es.<region>.amazonaws.com' constrained to domainARN=<domain arn> appeared on <key arn> within 600s; grants were []"

Expected: kms ListGrants(KeyId=<key id>) returns a grant with GranteePrincipal == RetiringPrincipal == 'es.<region>.amazonaws.com', Operations superset of {Decrypt, GenerateDataKey}, IssuingAccount == 'arn:aws:iam::<account>:root', and Constraints.EncryptionContextSubset.domainARN == 'arn:aws:es:<region>:<account>:domain/<domain>'.
Actual: ListGrants on the key returns an empty Grants list indefinitely — CreateDomain persists the EncryptionAtRestOptions on the domain record and makes no KMS call, so the poll never terminates and the check is killed at the harness cap. No error code or non-2xx status is involved: both CreateDomain and ListGrants answer 200, they just disagree with AWS about the side effect.

## Steps to Reproduce
1. kms CreateKey (symmetric, customer-managed); note Arn and KeyId; confirm KeyMetadata.KeyManager == 'CUSTOMER'.
2. opensearch CreateDomain(DomainName=<name>, EngineVersion='OpenSearch_2.11', ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':1}, EBSOptions={'EBSEnabled':True,'VolumeType':'gp3','VolumeSize':10}, EncryptionAtRestOptions={'Enabled':True,'KmsKeyId':<key arn>}, AccessPolicies=<account-root es:* policy for the domain ARN>).
3. kms ListGrants(KeyId=<key id>) (paginating on Truncated/NextMarker) — observe an empty Grants list, where AWS returns a grant to es.<region>.amazonaws.com constrained by Constraints.EncryptionContextSubset.domainARN to the new domain's ARN, with Operations including Decrypt and GenerateDataKey, RetiringPrincipal es.<region>.amazonaws.com and IssuingAccount arn:aws:iam::<account>:root.
4. Replay kms__opensearch.xsvc-opensearch-3-cmk-domain-echoes-key-grants-and-serves-search and confirm it no longer hits the 60 s cap and proceeds past the grant poll.
