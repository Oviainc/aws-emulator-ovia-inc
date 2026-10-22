Issue-Source: trace-sweep-divergence
Issue-Trace: vpclattice-service-network-lifecycle
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: vpclattice-service-network-lifecycle vpclattice-service-network-service-association vpclattice-service-and-listener-lifecycle
Issue-Shape-Id: com.amazonaws.vpclattice#MercuryControlPlane
Issue-Key: vpc-lattice|GetServiceNetwork|4xx->4xx|$.message
Issue-Key: vpc-lattice|GetServiceNetwork|4xx->4xx|$.resourceType
Issue-Key: vpc-lattice|CreateServiceNetwork|4xx->4xx|$.message
Issue-Key: vpc-lattice|CreateServiceNetwork|4xx->4xx|$.resourceType
Issue-Key: vpc-lattice|CreateServiceNetworkServiceAssociation|4xx->4xx|$.message
Issue-Key: vpc-lattice|CreateServiceNetworkServiceAssociation|4xx->4xx|$.resourceType
Issue-Key: vpc-lattice|DeleteServiceNetwork|4xx->4xx|$.resourceType
Issue-Key: vpc-lattice|GetServiceNetworkServiceAssociation|4xx->4xx|$.message
Issue-Key: vpc-lattice|GetServiceNetworkServiceAssociation|4xx->4xx|$.resourceType
Issue-Key: vpc-lattice|GetService|4xx->4xx|$.message
Issue-Key: vpc-lattice|GetService|4xx->4xx|$.resourceType
Issue-Key: vpc-lattice|CreateService|4xx->4xx|$.message
Issue-Key: vpc-lattice|CreateService|4xx->4xx|$.fieldList[*].message
Issue-Key: vpc-lattice|CreateServiceNetwork|4xx->4xx|$.fieldList[*].message
Issue-Commit: 38b62e174af309db3e46216fa3b3f7ac7f6f5b84

# VPC Lattice error payloads: wrong resourceType token and non-AWS message construction

> _Note: this issue was identified against the code base at commit `38b62e174af309db3e46216fa3b3f7ac7f6f5b84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetServiceNetwork

## Description
The emulator's vpc-lattice error bodies carry the right HTTP status and the right error shape, but the structured members of those shapes are filled with internal emulator identifiers and ad-hoc prose instead of the values AWS puts there. 9 replayed divergence clusters, grouped into 3 causes below. ALL THREE causes must be closed before this issue is submitted.

Cause 1 of 3: ResourceNotFoundException/ConflictException.resourceType carries the emulator's internal lowercase resource key instead of AWS's resource-type token.
AWS: Smithy: ResourceNotFoundException and ConflictException both declare `resourceId` and `resourceType` as REQUIRED String members (com.amazonaws.vpclattice#ResourceNotFoundException / #ConflictException, documented as "The resource type."). The model does not enumerate the token set, so: recorded behavior only — every recorded vpc-lattice error response spells resourceType as an upper-snake resource-type token: `SERVICE_NETWORK` for a service network, `SERVICE` for a service, `SERVICE_NETWORK_SERVICE_ASSOCIATION` for a service-network/service association. Status and error shape are unchanged (404 ResourceNotFoundException, 409 ConflictException).
Emulator: it emits the lowercase, delimiter-free internal key instead — `servicenetwork`, `service`, `servicenetworkserviceassociation` — on both the 404 and the 409 paths, for all three resource kinds.
Why: resourceType is a structured, required field of the error, not prose. Clients branch on it (retry the association, re-create the network, surface "which thing was missing") and CloudFormation/Terraform-style resource providers match it against the resource type they asked for. A token that is neither the AWS spelling nor a documented value makes every such branch fall through to the generic path, so a client that would recover on real AWS fails here.
Fix: the service's shared error-construction/serialization path for ResourceNotFoundException and ConflictException — the resource kind must be rendered to the wire token, not reused from the internal registry key. It is one mapping shared by all three resource kinds, so one correction closes all six clusters.

Cause 2 of 3: ValidationException on a malformed name does not use AWS's validation-error message construction.
AWS: Smithy: CreateService.name targets ServiceName with @pattern "^(?!svc-)(?![-])(?!.*[-]$)(?!.*[-]{2})[a-z0-9-]+$" (length 3..40); CreateServiceNetwork.name targets ServiceNetworkName with @pattern "^(?![-])(?!.*[-]$)(?!.*[-]{2})[a-z0-9-]+$" (length 3..63). ValidationException declares `message` (required), `reason` (required) and `fieldList` of ValidationExceptionField{name,message}. The recorded responses build the message from the request and the violated constraint, which is the standard AWS validation-error construction: message = "1 validation error detected: Value '<rejected value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", and fieldList[0].message = the same "Value ... failed to satisfy constraint: ..." text without the leading count. The rule is therefore known: the text is derived from the member name, the submitted value and the modeled pattern — not free prose.
Emulator: status 400 and the ValidationException shape are correct, but it substitutes hand-written prose that names neither the submitted value nor the constraint: message "name must not begin with the reserved prefix svc-" / "name may contain only lowercase letters, digits and hyphens", with fieldList[0].message "must not begin with the reserved prefix svc-" / "may contain only lowercase letters, digits and hyphens".
Why: this is the one error family where the message is machine-consumed: SDK/CLI users and test harnesses parse the "N validation error detected:" preamble and the "Value '<v>' at '<member>' failed to satisfy constraint:" fragment to report which field and which value was rejected, and the count tells a client how many fields are bad. Emulator text supplies neither the value nor the pattern, so a client cannot tell the user what to change and any assertion on the AWS format fails.
Fix: the validation layer for CreateService/CreateServiceNetwork name checks — emit the AWS validation-error construction from (member path, submitted value, violated constraint) and reuse it for every constrained member, rather than writing a bespoke sentence per check.

Cause 3 of 3: ConflictException/ResourceNotFoundException messages are not built the way AWS builds them.
AWS: recorded behavior only (the model types `message` as a required String and says nothing about its text). The recordings show a uniform construction derived from the request: a duplicate-name conflict reads "Resource with name <name> already exists in account <accountId>."; a duplicate association conflict reads "Association for <serviceNetworkArn> and <serviceArn> already exists."; a missing resource reads "<ResourceType> <id> cannot be found." (e.g. "Service svc-... cannot be found.", "ServiceNetwork sn-... cannot be found.", "Association snsa-... cannot be found."). Statuses are 409 and 404 respectively and match.
Emulator: it writes a lowercase sentence of its own that omits the account and the ARNs and uses internal naming: "a service named <name> already exists in this account", "a service network named <name> already exists in this account", "service <id> is already associated with service network <id>", "service svc-... not found", "service network sn-... not found", "service network service association snsa-... not found".
Why: the recorded AWS text identifies the conflicting resource by the identifiers the caller passed (and the account), which is what a caller logs or shows an operator; the emulator's text drops them. Treat the exact wording as an example, not a requirement — but the construction (include the name/ARNs/id and the account for a conflict, and the identifier for a not-found) is what should be reproduced.
Fix: same shared error-construction path as Cause 1 — format conflict and not-found messages from the request identifiers.

Note: the two open issues for this service (vpc-lattice CreateListener stub, vpc-lattice ListServiceNetworkServiceAssociations stub) cover unimplemented operations and do not cover any cluster above.

## Evidence
Replay of simparity-wire-aws-v1 (6 fixtures, 81 steps, 15 passed, score 18.5%; emulator up, 0 state_reset_failures).

Cause 1 — resourceType token (6 clusters, status matched in all):
- vpclattice-service-network-lifecycle#task#5 (GetServiceNetwork, 404; also affects #task#13): $.resourceType expected 'SERVICE_NETWORK', got 'servicenetwork'.
- vpclattice-service-network-lifecycle#task#3 (CreateServiceNetwork, 409): $.resourceType expected 'SERVICE_NETWORK', got 'servicenetwork'.
- vpclattice-service-network-service-association#task#8 (DeleteServiceNetwork, 409): $.resourceType expected 'SERVICE_NETWORK', got 'servicenetwork' (resourceType is the ONLY divergence in this cluster).
- vpclattice-service-network-service-association#task#6 (CreateServiceNetworkServiceAssociation, 409): $.resourceType expected 'SERVICE_NETWORK_SERVICE_ASSOCIATION', got 'servicenetworkserviceassociation'.
- vpclattice-service-network-service-association#task#10 (GetServiceNetworkServiceAssociation, 404): $.resourceType expected 'SERVICE_NETWORK_SERVICE_ASSOCIATION', got 'servicenetworkserviceassociation'.
- vpclattice-service-and-listener-lifecycle#task#14 (GetService, 404; also affects #task#15): $.resourceType expected 'SERVICE', got 'service'.

Cause 2 — ValidationException construction (2 clusters, both status 400):
- vpclattice-service-and-listener-lifecycle#task#4 (CreateService, name 'svc-sim2real-probe-b52d9f07'): $.message expected "1 validation error detected: Value 'svc-sim2real-probe-b52d9f07' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: (?!svc-)(?![-])(?!.*[-]$)(?!.*[-]{2})[a-z0-9-]+", got 'name must not begin with the reserved prefix svc-'; $.fieldList[0].message expected the same text without the count prefix, got 'must not begin with the reserved prefix svc-'.
- vpclattice-service-network-lifecycle#task#4 (CreateServiceNetwork, name 'sim2real-probe-LAT-BAD-a71c4e33'): $.message expected "1 validation error detected: Value 'sim2real-probe-LAT-BAD-a71c4e33' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: (?![-])(?!.*[-]$)(?!.*[-]{2})[a-z0-9-]+", got 'name may contain only lowercase letters, digits and hyphens'; $.fieldList[0].message likewise.

Cause 3 — message construction on 409/404 (message text shown as examples of the construction):
- vpclattice-service-and-listener-lifecycle#task#3 (CreateService, 409): expected 'Resource with name sim2real-probe-lat-svc-b52d9f07 already exists in account 000000000000.', got 'a service named sim2real-probe-lat-svc-b52d9f07 already exists in this account' (message is the only divergence in this cluster).
- vpclattice-service-network-lifecycle#task#3 (409): expected 'Resource with name sim2real-probe-lat-sn-a71c4e33 already exists in account 000000000000.', got 'a service network named ... already exists in this account'.
- vpclattice-service-network-service-association#task#6 (409): expected 'Association for <serviceNetworkArn> and <serviceArn> already exists.', got 'service svc-... is already associated with service network sn-...'.
- vpclattice-service-and-listener-lifecycle#task#14 (404): expected 'Service svc-... cannot be found.', got 'service svc-... not found'.
- vpclattice-service-network-lifecycle#task#5 (404): expected 'ServiceNetwork sn-0123456789abcdefg cannot be found.', got 'service network sn-0123456789abcdefg not found'.
- vpclattice-service-network-service-association#task#10 (404): expected 'Association snsa-... cannot be found.', got 'service network service association snsa-... not found'.

## Steps to Reproduce
1. Replay trace set simparity-wire-aws-v1 against the emulator.
2. Cause 1: GET /servicenetworks/sn-0123456789abcdefg for an id that does not exist (vpclattice-service-network-lifecycle#task#5) and inspect the 404 body — $.resourceType is 'servicenetwork' instead of 'SERVICE_NETWORK'. Repeat on the 409 paths: re-POST /servicenetworks with an existing name (#task#3), re-POST the same service-network/service association (vpclattice-service-network-service-association#task#6), DELETE a service network that still has an association (#task#8), and GET a deleted service/association (vpclattice-service-and-listener-lifecycle#task#14, vpclattice-service-network-service-association#task#10).
3. Cause 2: POST /services with name 'svc-sim2real-probe-b52d9f07' (reserved prefix) and POST /servicenetworks with name 'sim2real-probe-LAT-BAD-a71c4e33' (uppercase) and inspect $.message and $.fieldList[0].message of the 400 ValidationException.
4. Cause 3: trigger each 409/404 above and inspect $.message.
5. Re-replay the fixtures/steps named under Traces; all 9 clusters must pass.
