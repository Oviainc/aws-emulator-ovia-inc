Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 0761ec9a912f1ad457725ac2bfa14369ca92ad04

# Bootstrap the pca-connector-scep emulator

_Filed: 2026-10-21T23:12:55Z_
_Operation: service-com-amazonaws-pcaconnectorscep-pcaconnectorscep | Class: stub_

> _Note: this issue was identified against the code base at commit `0761ec9a912f1ad457725ac2bfa14369ca92ad04`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/ops/` is an unmodified scaffold (`//! SCAFFOLDED, NOT IMPLEMENTED:` header, returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501) and the service holds no state. Implement a FIRST stateful slice: the connector + challenge lifecycle, wire protocol `restJson1`, sigv4 name `pca-connector-scep`.

### Happy path to implement (in order)

1. **CreateConnector** — `POST /connectors` -> 202. Input: `CertificateAuthorityArn` (required, acm-pca CA ARN), optional `MobileDeviceManagement` (union with `Intune`), `VpcEndpointId`, `ClientToken` (@idempotencyToken), `Tags`. Must mint a fresh connector ARN of the modeled form `arn:aws:pca-connector-scep:<region>:<account>:connector/<uuid>`, persist a full connector record, and return `ConnectorArn`. Derive `Type` from input: `GENERAL_PURPOSE` when `MobileDeviceManagement` is absent, `INTUNE` when `Intune` is supplied. Honour `ClientToken`: a repeat call with the same token and same inputs returns the SAME `ConnectorArn` instead of creating a second connector.
2. **GetConnector** — `GET /connectors/{ConnectorArn}` -> 200. Must return the stored `Connector` built from state: `Arn`, `CertificateAuthorityArn`, `Type`, `MobileDeviceManagement` (echo of what was stored, empty/absent for general purpose), `OpenIdConfiguration` (only for INTUNE connectors), `Status` (`ACTIVE` once created; model allows `CREATING`/`DELETING`/`FAILED`), `Endpoint` (the connector's HTTPS SCEP URL, derived from the connector id), `CreatedAt`, `UpdatedAt`. Unknown/ deleted ARN -> `ResourceNotFoundException` (404), not 200 with an empty body.
3. **CreateChallenge** — `POST /challenges` -> 202. Input: `ConnectorArn` (required), `ClientToken`, `Tags`. Must look the connector up in state and 404 (`ResourceNotFoundException`) when it does not exist. On success generate a challenge id and password (UUID format, `Password` is a sensitive string), persist them under the connector, and return a `Challenge` with `Arn` of the modeled form `.../connector/<uuid>/challenge/<uuid>`, `ConnectorArn`, `CreatedAt`, `UpdatedAt`, `Password`. Same `ClientToken` within the idempotency window -> same challenge.
4. **GetChallengePassword** — `GET /challengePasswords/{ChallengeArn}` -> 200, returning `Password` read back from state; it must byte-for-byte equal the password returned by CreateChallenge. Unknown ARN -> 404.
5. **ListChallengeMetadata** — `GET /challengeMetadata?ConnectorArn=...` -> 200. `ConnectorArn` is required; returns `Challenges` as a list of `ChallengeMetadataSummary` (`Arn`, `ConnectorArn`, `CreatedAt`, `UpdatedAt`) for exactly that connector, filtered from state, honouring `MaxResults` (1..1000) with an opaque base64-ish `NextToken` round-trip (token pattern `^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2}==|[A-Za-z0-9_-]{3}=)?$`). The password must NOT appear in metadata responses.
6. **DeleteChallenge** / **DeleteConnector** — `DELETE /challenges/{ChallengeArn}` -> 202 and `DELETE /connectors/{ConnectorArn}` -> 202, both @idempotent. Must actually remove the record so a subsequent GetChallengePassword / GetConnector answers `ResourceNotFoundException` (404), and so the entry disappears from ListChallengeMetadata / ListConnectors. Deleting a connector must also drop its challenges.

**ListConnectors** (`GET /connectors`, paginated over `ConnectorSummary`) should be included in this slice as the listing counterpart of CreateConnector: it must enumerate connectors from state with `MaxResults`/`NextToken` honoured.

Every one of these must compute its response from persisted state. No canned ARNs, no echoing the request back, no random id that is never stored: a create followed by a get/list must return exactly what was created, and a delete must be observable by the next reader.

## State Model

```
ScepState {
    connectors: Map<String /* connector ARN */, Connector>,
    // ClientToken -> created connector ARN, for CreateConnector idempotency
    connector_tokens: Map<String, String>,
    // ClientToken -> created challenge ARN, for CreateChallenge idempotency
    challenge_tokens: Map<String, String>,
    // resource ARN (connector or challenge) -> tag map, for Tag/Untag/ListTagsForResource
    tags: Map<String, Map<String, String>>,
}

Connector {
    arn: String,                       // arn:aws:pca-connector-scep:<region>:<acct>:connector/<uuid>
    certificate_authority_arn: String,
    r#type: ConnectorType,             // GENERAL_PURPOSE | INTUNE
    mobile_device_management: Option<MobileDeviceManagement>,
    open_id_configuration: Option<OpenIdConfiguration>,  // INTUNE only
    vpc_endpoint_id: Option<String>,
    status: ConnectorStatus,           // CREATING | ACTIVE | DELETING | FAILED
    status_reason: Option<ConnectorStatusReason>,
    endpoint: String,                  // HTTPS SCEP URL derived from connector id
    created_at: Timestamp,
    updated_at: Timestamp,
    challenges: Map<String /* challenge ARN */, Challenge>,
}

Challenge {
    arn: String,          // <connector arn>/challenge/<uuid>
    connector_arn: String,
    password: String,     // UUID-format, sensitive: only returned by CreateChallenge
                          // and GetChallengePassword
    created_at: Timestamp,
    updated_at: Timestamp,
}
```

Out of scope for this first slice (stays 501, filed separately later): GetChallengeMetadata, TagResource, UntagResource, ListTagsForResource.

## Evidence

OPERATIONS.md at the crate root lists all 12 operations with routes, members and errors: CreateConnector `POST /connectors -> 202` (required `CertificateAuthorityArn`, output `ConnectorArn`), GetConnector `GET /connectors/{ConnectorArn} -> 200` (output `Connector`), CreateChallenge `POST /challenges -> 202` (required `ConnectorArn`, output `Challenge`), GetChallengePassword `GET /challengePasswords/{ChallengeArn} -> 200` (output `Password`), ListChallengeMetadata `GET /challengeMetadata -> 200` (@paginated inputToken=NextToken/outputToken=NextToken/items=Challenges, required query `ConnectorArn`), ListConnectors `GET /connectors -> 200` (@paginated, items=Connectors), DeleteChallenge/DeleteConnector `DELETE ... -> 202` (@idempotent). Shape detail from smithy_query: `Connector{Arn,CertificateAuthorityArn,Type,MobileDeviceManagement,OpenIdConfiguration,Status,StatusReason,Endpoint,CreatedAt,UpdatedAt}`, `Challenge{Arn,ConnectorArn,CreatedAt,UpdatedAt,Password(sensitive)}`, `ChallengeMetadataSummary{Arn,ConnectorArn,CreatedAt,UpdatedAt}`, `ConnectorStatus=[CREATING,ACTIVE,DELETING,FAILED]`, `ConnectorType=[GENERAL_PURPOSE,INTUNE]`, `MobileDeviceManagement` is a union with member `Intune`. All six listed handlers are scaffolds: e.g. `src/ops/create_connector.rs` lines 4 and 208 show the `SCAFFOLDED, NOT IMPLEMENTED:` header and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateConnector"))`. No tests exercise any of this.

## Steps to Reproduce

1. CreateConnector with a valid acm-pca `CertificateAuthorityArn` and no `MobileDeviceManagement` -> expect 202 and a `ConnectorArn` matching `arn:aws:pca-connector-scep:<region>:<account>:connector/<uuid>`; today: HTTP 501 NotImplemented.
2. Repeat the same CreateConnector call with the identical `ClientToken` -> expect the SAME `ConnectorArn` back and still only one connector in state.
3. GetConnector with that ARN -> expect 200 with `Connector.CertificateAuthorityArn` equal to the input, `Type=GENERAL_PURPOSE`, `Status=ACTIVE`, a non-empty `Endpoint`, and `CreatedAt`/`UpdatedAt` set.
4. ListConnectors -> expect the new connector present in `Connectors`; with `MaxResults=1` and more than one connector expect a `NextToken` that pages to the remainder without duplicates.
5. CreateChallenge with that `ConnectorArn` -> expect 202 with `Challenge.Arn` of the form `<connectorArn>/challenge/<uuid>` and a UUID-format `Password`.
6. GetChallengePassword with the challenge ARN -> expect 200 and the exact same `Password` string returned by CreateChallenge.
7. ListChallengeMetadata?ConnectorArn=<connectorArn> -> expect the challenge listed as a `ChallengeMetadataSummary` with no password field; a different connector's ARN must not list it.
8. DeleteChallenge on the challenge ARN -> 202; then GetChallengePassword on the same ARN -> expect `ResourceNotFoundException` (404), and ListChallengeMetadata no longer lists it. Repeat DeleteChallenge -> still succeeds (idempotent).
9. DeleteConnector on the connector ARN -> 202; then GetConnector -> expect `ResourceNotFoundException` (404) and ListConnectors no longer returns it.
10. CreateChallenge against a well-formed but nonexistent connector ARN -> expect `ResourceNotFoundException` (404), not 202.
