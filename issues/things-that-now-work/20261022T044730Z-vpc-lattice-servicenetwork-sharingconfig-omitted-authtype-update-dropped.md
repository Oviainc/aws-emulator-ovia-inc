Issue-Source: trace-sweep-divergence
Issue-Trace: vpclattice-service-network-lifecycle
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: vpclattice-service-network-lifecycle vpclattice-service-and-listener-lifecycle vpclattice-service-network-service-association
Issue-Shape-Id: com.amazonaws.vpclattice#MercuryControlPlane
Issue-Key: vpc-lattice|GetServiceNetwork|2xx->2xx|$.authType
Issue-Key: vpc-lattice|GetServiceNetwork|2xx->2xx|$.createdAt
Issue-Key: vpc-lattice|GetServiceNetwork|2xx->2xx|$.lastUpdatedAt
Issue-Key: vpc-lattice|GetServiceNetwork|2xx->2xx|$.sharingConfig
Issue-Key: vpc-lattice|CreateServiceNetwork|2xx->2xx|$.sharingConfig
Issue-Key: vpc-lattice|GetService|2xx->2xx|$.arn
Issue-Key: vpc-lattice|GetService|2xx->2xx|$.createdAt
Issue-Key: vpc-lattice|GetService|2xx->2xx|$.dnsEntry.domainName
Issue-Key: vpc-lattice|GetService|2xx->2xx|$.lastUpdatedAt
Issue-Key: vpc-lattice|GetServiceNetworkServiceAssociation|2xx->2xx|$.arn
Issue-Key: vpc-lattice|GetServiceNetworkServiceAssociation|2xx->2xx|$.createdAt
Issue-Key: vpc-lattice|GetServiceNetworkServiceAssociation|2xx->2xx|$.dnsEntry.domainName
Issue-Key: vpc-lattice|GetServiceNetworkServiceAssociation|2xx->2xx|$.id
Issue-Key: vpc-lattice|GetServiceNetworkServiceAssociation|2xx->2xx|$.serviceArn
Issue-Key: vpc-lattice|GetServiceNetworkServiceAssociation|2xx->2xx|$.serviceId
Issue-Key: vpc-lattice|GetServiceNetworkServiceAssociation|2xx->2xx|$.serviceNetworkArn
Issue-Key: vpc-lattice|GetServiceNetworkServiceAssociation|2xx->2xx|$.serviceNetworkId
Issue-Commit: 38b62e174af309db3e46216fa3b3f7ac7f6f5b84

# VPC Lattice ServiceNetwork: sharingConfig omitted, authType update dropped, timestamps emitted as epoch numbers

> _Note: this issue was identified against the code base at commit `38b62e174af309db3e46216fa3b3f7ac7f6f5b84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetServiceNetwork

## Description
Successful (2xx) vpc-lattice ServiceNetwork responses are missing a modeled output member, lose a mutation that was accepted, and serialize timestamps in the wrong format. 5 replayed divergence clusters, grouped into 3 causes below. ALL THREE causes must be closed before this issue is submitted.

Cause 1 of 3: UpdateServiceNetwork's authType change is not persisted, so a later GetServiceNetwork still reports the old value.
AWS: Smithy: UpdateServiceNetwork (PATCH /servicenetworks/{serviceNetworkIdentifier}) takes `authType` as a REQUIRED member targeting AuthType (values NONE | AWS_IAM) and returns the updated authType; GetServiceNetwork returns `authType` from the stored resource. An accepted update is therefore readable: after UpdateServiceNetwork sets authType, GetServiceNetwork must report the new value. The recording shows exactly that — the update is accepted and the subsequent GetServiceNetwork reports authType 'NONE' with status 200.
Emulator: the update call succeeds, but the following GetServiceNetwork (status 200, as expected) still reports authType 'AWS_IAM' — the value the network was created with. The mutation is silently discarded: nothing in the response tells the caller the change did not take.
Why: this is a silent lost write on an authorization setting. A caller that switches a service network to NONE (or to AWS_IAM) gets a success response and then reads back the opposite policy mode, so config-convergence loops never settle and a test that asserts the hardened setting passes against state that was never changed. Worst case the emulator reports AWS_IAM while callers behave as if the network were open, or vice versa.
Fix: UpdateServiceNetwork's handler/state write — apply authType to the stored service network (and refresh lastUpdatedAt) instead of only echoing it in the response.

Cause 2 of 3: sharingConfig is absent from CreateServiceNetwork and GetServiceNetwork responses.
AWS: Smithy: CreateServiceNetworkResponse and GetServiceNetworkResponse both declare `sharingConfig` targeting SharingConfig, a structure with the boolean member `enabled` ("Specifies if the service network is enabled for sharing"); CreateServiceNetworkRequest accepts the same member. The recording shows both the 201 create and the 200 get returning sharingConfig {"enabled": true}.
Emulator: the member is omitted entirely from both responses (replay reports `$.sharingConfig: missing (expected {'enabled': True}, got None)`), on create and on every subsequent read — so the setting is neither echoed nor stored.
Why: a client cannot discover whether the service network is shareable. SDK deserialization yields an absent optional, so code that reads sharingConfig.enabled sees nothing rather than the value it just set, and an infrastructure tool that diffs desired vs actual state sees the field as perpetually unset and re-applies forever.
Fix: CreateServiceNetwork's handler/state (persist the requested sharingConfig, defaulting as AWS does) and the CreateServiceNetwork/GetServiceNetwork serializers (emit the member).

Cause 3 of 3: createdAt / lastUpdatedAt are serialized as JSON numbers instead of the modeled date-time strings.
AWS: Smithy: the `createdAt` and `lastUpdatedAt` members of GetServiceNetworkResponse, GetServiceResponse and GetServiceNetworkServiceAssociationResponse target Timestamp carrying @timestampFormat("date-time"), i.e. an RFC 3339 / ISO-8601 string on the wire, overriding the restJson1 epoch-seconds default.
Emulator: it emits a bare fractional epoch number — e.g. `$.createdAt`/`$.lastUpdatedAt` came back as 1792644250.6097262 on GetServiceNetwork, 1792644244.87262 on GetService and 1792644256.4961195 on GetServiceNetworkServiceAssociation — for every timestamp member of those reads. Status 200 is correct; only the encoding of the member differs.
Why: a strict restJson1 client deserializing a @timestampFormat("date-time") member expects a string, so a number is a deserialization error or a wrong instant, and anything that prints or compares the timestamp textually breaks. The exact instants are environment-specific and are NOT a requirement — only the encoding is.
Fix: the vpc-lattice response serializers (the shared timestamp rendering used by GetServiceNetwork, GetService and GetServiceNetworkServiceAssociation) — honour @timestampFormat("date-time").

## Evidence
Replay of simparity-wire-aws-v1 (6 fixtures, 81 steps, 15 passed, score 18.5%; emulator up, 0 state_reset_failures). All clusters below returned the EXPECTED status; the divergences are in the response body.

Cause 1 — authType lost:
- vpclattice-service-network-lifecycle#task#7 (GetServiceNetwork, 200): $.authType expected 'NONE', got 'AWS_IAM'. The same fixture's earlier GetServiceNetwork (#task#2) shows no authType divergence, so the create-time value matched and only the update was dropped.

Cause 2 — sharingConfig missing:
- vpclattice-service-network-lifecycle#task#1 (CreateServiceNetwork, 201): $.sharingConfig: missing (expected {'enabled': True}, got None).
- vpclattice-service-network-lifecycle#task#2 (GetServiceNetwork, 200): $.sharingConfig: missing (expected {'enabled': True}, got None).
- vpclattice-service-network-lifecycle#task#7 (GetServiceNetwork, 200): $.sharingConfig: missing (expected {'enabled': True}, got None).

Cause 3 — epoch-number timestamps where the model declares date-time:
- vpclattice-service-network-lifecycle#task#2 and #task#7 (GetServiceNetwork, 200): $.createdAt and $.lastUpdatedAt returned as 1792644250.6097262.
- vpclattice-service-and-listener-lifecycle#task#2 (GetService, 200): $.createdAt and $.lastUpdatedAt returned as 1792644244.87262.
- vpclattice-service-network-service-association#task#5 (GetServiceNetworkServiceAssociation, 200): $.createdAt returned as 1792644256.4961195.
(The numeric values are examples from this run; the defect is the JSON number encoding, not the instant.)

## Steps to Reproduce
1. Replay trace set simparity-wire-aws-v1 against the emulator.
2. Cause 1: POST /servicenetworks creating a network with authType AWS_IAM, then PATCH /servicenetworks/{id} with authType NONE (accepted), then GET /servicenetworks/{id} — vpclattice-service-network-lifecycle#task#7 returns authType 'AWS_IAM' instead of the updated 'NONE'.
3. Cause 2: inspect the 201 body of the create (#task#1) and the 200 body of each GET (#task#2, #task#7) — sharingConfig is absent in all three, instead of {"enabled": true}.
4. Cause 3: inspect $.createdAt / $.lastUpdatedAt in GetServiceNetwork (#task#2, #task#7), GetService (vpclattice-service-and-listener-lifecycle#task#2) and GetServiceNetworkServiceAssociation (vpclattice-service-network-service-association#task#5) — each is a JSON number rather than an RFC 3339 string.
5. Re-replay the fixtures/steps named under Traces; all 5 clusters must pass.
