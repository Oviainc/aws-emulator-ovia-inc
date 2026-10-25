Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.op-weighted-rrset-without-set-identifier-rejected route53.rrset-weighted-siblings-require-distinct-set-identifiers
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.op-weighted-rrset-without-set-identifier-rejected|run#10|AssertionError|assert code == "InvalidInput", (
Issue-Key: route53.rrset-weighted-siblings-require-distinct-set-identifiers|run#24|AssertionError|assert msg == (
Issue-Commit: df6081fa6700f84ddafc058f223e27db3aea4084

# ChangeResourceRecordSets collapses distinct record-set refusal rules into one generic name+type conflict (wrong error code on the missing-SetIdentifier rule)

> _Note: this issue was identified against the code base at commit `df6081fa6700f84ddafc058f223e27db3aea4084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ChangeResourceRecordSets

## Description
REOPENS 20261024T114159Z-route53-route-53-record-set-representation-dns-names-are.md: its fix left the call refused. That issue carried route53.rrset-weighted-siblings-require-distinct-set-identifiers and claimed the ChangeResourceRecordSets refusal discriminators; the check still fails on a ChangeResourceRecordSets refusal, now on the simple-vs-routed-group rule, and the sibling check route53.op-weighted-rrset-without-set-identifier-rejected fails on the same handler with the wrong error CODE. Both surviving failures come from one unimplemented distinction: the handler answers every record-set collision at a (Name, Type) with a single generic 'conflicts with other records with the same DNS name and type' refusal, instead of evaluating the per-change rules AWS evaluates first.

TWO CAUSES are bundled here; both must be closed.

Cause 1 of 2: a change that supplies Weight but omits SetIdentifier is refused as a name+type conflict (InvalidChangeBatch) instead of the modeled missing-required-field refusal (InvalidInput).
AWS: Smithy ChangeResourceRecordSets binds both InvalidInput (client error, HTTP 400) and InvalidChangeBatch (client error, HTTP 400); routing-policy field completeness is validated per change BEFORE collision detection, so a CREATE carrying Weight with no SetIdentifier is refused with InvalidInput and a message naming the missing member and the change's identity — recorded form: "Invalid request: Missing field 'SetIdentifier' in Change with [Action=CREATE, Name=<name>, Type=A, SetIdentifier=null]". Rule: code InvalidInput, message naming the absent member and the change it belongs to.
Emulator: HTTP 400 with code InvalidChangeBatch and message '[RRSet of type A with DNS name <name> is not permitted as it conflicts with other records with the same DNS name and type in zone <zone>]' — the generic collision refusal. The code itself is wrong, not just the text.
Why: the two codes mean different things to a client. InvalidInput says 'your change is malformed, fix the request'; InvalidChangeBatch says 'the change is well-formed but conflicts with zone state, look at what is already there'. An SDK/IaC error handler that retries or reconciles on a state conflict loops forever on a request that can never succeed, and nothing in the response names the member the caller actually left out.
Fix: services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/change_resource_record_sets.rs — validate per-change routing-policy field completeness (Weight/Region/GeoLocation/Failover present => SetIdentifier required) and raise InvalidInput naming the missing member, before the (Name, Type) collision check runs.
Traces: route53.op-weighted-rrset-without-set-identifier-rejected. (Its positive control passed: two weighted siblings with distinct SetIdentifiers are created and round-trip, so this is about the missing field specifically.)

Cause 2 of 2: a SIMPLE record created at a name+type that already holds a routed group is refused with the duplicate/collision wording instead of the distinct simple-vs-routed-group rule.
AWS: InvalidChangeBatch (client error, HTTP 400) — the code and the effect (the record does not join the group) match. AWS states the zone-level rule that was violated, in its own wording that notably does not mention SetIdentifier at all; recorded form: '[RRSet with DNS name <name>, type A cannot be created as other RRSets exist with the same name and type.]' Rule: this refusal is reported distinctly from the duplicate-CREATE refusal (which echoes [name=..., type=..., set-identifier=...]).
Emulator: '[RRSet of type A with DNS name <name> is not permitted as it conflicts with other records with the same DNS name and type in zone <zone>]' — the same string it returns for Cause 1, i.e. one message for several distinct rules.
Why: a client parsing the InvalidChangeBatch message to tell 'you may not mix simple and routed records at this name' from 'this exact record set already exists' (two different remediations: delete the group vs. pick a new SetIdentifier) cannot distinguish them, and the collapse indicates the rules themselves are not separately evaluated.
Fix: same handler — give the simple-vs-routed-group branch its own refusal, separate from the duplicate-CREATE branch.
Traces: route53.rrset-weighted-siblings-require-distinct-set-identifiers. (Its earlier assertions now pass; the control at the end — a third distinct SetIdentifier being accepted — was never reached.)

Related but NOT covering these: open issue 20261024T114159Z-route53-changeresourcerecordsets-performs-no-record-set-validation-seven-classes.md owns the record-set validation classes that are ACCEPTED with 200 PENDING; both failures here are refusals that do happen, with the wrong code or the wrong rule.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (route53 serving, 23/114 checks passing).

Cause 1 — AssertionError: expected InvalidInput for weighted rrset without SetIdentifier, got InvalidChangeBatch: [RRSet of type A with DNS name weighted.sim2real-behav-chk-op-weightd425de6f81c0.sim2real-probe-zone.com. is not permitted as it conflicts with other records with the same DNS name and type in zone sim2real-behav-chk-op-weightd425de6f81c0.sim2real-probe-zone.com.]
Expected (recorded AWS): code InvalidInput, message "Invalid request: Missing field 'SetIdentifier' in Change with [Action=CREATE, Name=weighted.<zone>, Type=A, SetIdentifier=null]".

Cause 2 — AssertionError: expected the simple-vs-routed-group wording, got '[RRSet of type A with DNS name wt.sim2real-behav-zone7632cb701388.example.test. is not permitted as it conflicts with other records with the same DNS name and type in zone sim2real-behav-zone7632cb701388.example.test.]'
Expected (recorded AWS, us-west-2): '[RRSet with DNS name wt.<zone>, type A cannot be created as other RRSets exist with the same name and type.]' (code InvalidChangeBatch in both).

Model evidence (smithy_query on services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json): ChangeResourceRecordSets errors = InvalidChangeBatch (client, 400, members messages:ErrorMessages + message), InvalidInput (client, 400), NoSuchHealthCheck (404), NoSuchHostedZone (404), PriorRequestNotComplete (400).

## Steps to Reproduce
1. CreateHostedZone (public). CREATE two weighted A records at weighted.<zone> with SetIdentifiers 'blue' and 'green' — this succeeds.
2. Cause 1: CREATE a third A record at the same name with Weight=50 and NO SetIdentifier. Observe code InvalidChangeBatch with the generic name+type conflict message instead of InvalidInput naming the missing 'SetIdentifier' field.
3. Cause 2: in a zone that already holds a weighted A group at wt.<zone>, CREATE a SIMPLE A record (no SetIdentifier, no Weight) at wt.<zone>. Observe the identical generic conflict message instead of AWS's '[RRSet with DNS name ..., type A cannot be created as other RRSets exist with the same name and type.]'.
