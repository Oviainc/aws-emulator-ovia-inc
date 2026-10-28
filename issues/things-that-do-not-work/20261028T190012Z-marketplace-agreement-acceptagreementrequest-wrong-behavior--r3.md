Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7cfd583752b909e6f589468ac42594db7887f86f

# Entitlement licence ARNs are minted from the raw SigV4 credential-scope region, so a stored licenceArn can be one AwsArn's pattern forbids and SearchAgreements' LicenseArn filter can never carry

_Filed: 2026-10-28T19:00:12Z_
_Operation: AcceptAgreementRequest | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7cfd583752b909e6f589468ac42594db7887f86f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Accepting an agreement provisions one entitlement per resource and mints each entitlement's `licenseArn` by interpolating `crate::state::request_region` verbatim into `::engine::emulator_arn`. `request_region` returns the region field of the caller's SigV4 credential scope exactly as it was written in the `Authorization` header; the parser that produces it applies no character rule and no length rule — it only rejects an EMPTY field. The minted string is then persisted on the agreement's entitlement record and is the ONLY handle that identifies the licence.

The modelled member it is reported under, `AgreementEntitlement$licenseArn` (target `AwsArn`), restricts the region segment to `[A-Za-z0-9_/.-]{0,63}`. So a request signed for a credential scope whose region carries any other character (a space, `+`, `@`, `=`) or more than 63 characters makes `AcceptAgreementRequest` persist — and `GetAgreementEntitlements` / `SearchAgreements` then report — a `licenseArn` the member's own modelled pattern says cannot hold it. The helper that mints it states the opposite rule in its own doc comment: "Every ARN this mints satisfies `AwsArn`'s pattern".

The second half of the contradiction is inside this crate: `SearchAgreements`' `LicenseArn` filter matches a stored entitlement by exact string equality against a `FilterValue`, whose modelled pattern is `^[A-Za-z0-9.,+:/_-]+$`. An ARN containing a space (the region `us east 1`) can therefore never be passed as that filter's value — the boundary validator refuses it — so the agreement whose licence this emulator just minted is permanently unreachable through the one filter the model publishes for finding it, while every other agreement is reachable.

## Evidence

The minting helper asserts the rule it can break — `services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/state.rs:4371-4385`:
```
    /// Mint the ARN of the AWS License Manager licence one entitlement grants.
    ///
    /// The licence id carries License Manager's own `l-` prefix, and the ARN is
    /// built through [`::engine::emulator_arn`], so its account segment is the
    /// emulator's canonical one and its region is the one the accepting caller
    /// signed for. Every ARN this mints satisfies `AwsArn`'s pattern, whose
    /// resource part admits the `license:<id>` form License Manager uses.
    fn mint_license_arn(&mut self, region: &str) -> String {
        let license_id = format!("l-{:016x}", self.next_sequence());
        ::engine::emulator_arn(
            LICENSE_ARN_SERVICE,
            region,
            &format!("license:{license_id}"),
        )
    }
```
The region is the caller's header, unchecked — `src/ops/accept_agreement_request.rs:260-261`:
```
        let region = crate::state::request_region(ctx.parts());
        let entitlements = store.provision_entitlements(&resources, &region, now);
```
`src/state.rs:248-252`:
```
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}
```
and its producer rejects only emptiness — `vendor/server/src/lib.rs:1800-1808`:
```
    if access_key_id.is_empty() || date.is_empty() || region.is_empty() || service.is_empty() {
        return None;
    }
    Some(SigV4Scope {
        access_key_id: access_key_id.to_string(),
        date: date.to_string(),
        region: region.to_string(),
        service: service.to_string(),
    })
```
which is interpolated with no escaping — `vendor/server/src/lib.rs:162-164`:
```
pub fn emulator_arn(service: &str, region: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{region}:{EMULATOR_ACCOUNT_ID}:{resource}")
}
```
What the model allows that member to be — `smithy_query(query="shape", shape="AgreementEntitlement")`: `"licenseArn": {"target": "AwsArn", "length": {"min": 1, "max": 2048}, "pattern": "^arn:aws[a-zA-Z-]*:[A-Za-z0-9][A-Za-z0-9_/.-]{0,62}:[A-Za-z0-9_/.-]{0,63}:[A-Za-z0-9_/.-]{0,63}:[A-Za-z0-9][A-Za-z0-9:_/+=,@.-]{0,1023}$"}` — the region segment is `[A-Za-z0-9_/.-]{0,63}`.

The stored ARN is the only handle the search filter has — `src/state.rs:3130-3133`:
```
        "LicenseArn" => agreement
            .entitlements
            .iter()
            .any(|entitlement| entitlement.license_arn == value),
```
and that value must satisfy `smithy_query(query="shape", shape="FilterValueList")`: `"member": {"target": "FilterValue", "length": {"min": 1, "max": 256}, "pattern": "^[A-Za-z0-9.,+:/_-]+$"}`, which a space-bearing ARN cannot.

Test coverage: `tests/all/entitlement_resources.rs` and `tests/all/agreement_entitlements.rs` exercise entitlements only under a canonical credential scope; no test signs for a region outside the ARN region class, so nothing pins the current behaviour.

## Steps to Reproduce

1. CreateAgreementRequest (intent NEW, a proposal identifier, one requested term) and note the `agreementRequestId`.
2. Call AcceptAgreementRequest for that id with `Authorization: AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/us east 1/aws-marketplace/aws4_request, SignedHeaders=host, Signature=x` (the region field carries a space, which `sigv4_credential_scope` accepts because it is non-empty). HTTP 200, an agreement is minted.
3. Call GetAgreementEntitlements for that agreement. Observed: each entitlement reports `licenseArn` = `arn:aws:license-manager:us east 1:000000000000:license:l-...`. Compare with the modelled `AwsArn` pattern — the region segment `[A-Za-z0-9_/.-]{0,63}` cannot contain a space, so the handler emitted a value for a member the model says cannot hold it.
4. Call SearchAgreements with filters `[{"name":"AgreementType","values":["PurchaseAgreement"]},{"name":"LicenseArn","values":["arn:aws:license-manager:us east 1:000000000000:license:l-..."]}]`. Expected: the agreement just accepted. Actual: HTTP 400 ValidationException from the boundary walk, because `FilterValue`'s pattern `^[A-Za-z0-9.,+:/_-]+$` forbids the space — the licence this service minted can never be used as its own filter value.
5. Repeat step 2 with a 100-character region to see the same ARN exceed the region segment's 63-character bound.
