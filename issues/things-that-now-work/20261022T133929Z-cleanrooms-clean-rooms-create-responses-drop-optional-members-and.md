Issue-Source: trace-sweep-divergence
Issue-Trace: cleanrooms-change-request-cancel-workflow
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cleanrooms-change-request-cancel-workflow
Issue-Shape-Id: com.amazonaws.cleanrooms#AWSBastionControlPlaneServiceLambda
Issue-Key: cleanrooms|CreateCollaboration|2xx->2xx|$.collaboration.analyticsEngine
Issue-Key: cleanrooms|CreateCollaboration|2xx->2xx|$.collaboration.isMetricsEnabled
Issue-Key: cleanrooms|CreateCollaboration|2xx->2xx|$.collaboration.memberStatus
Issue-Key: cleanrooms|CreateMembership|2xx->2xx|$.membership.isMetricsEnabled
Issue-Key: cleanrooms|CreateMembership|2xx->2xx|$.membership.paymentConfiguration.machineLearning
Issue-Commit: 67b7a4ea014b7cfd9cdb5213b26731b0db184c45

# Clean Rooms create responses drop optional members and mis-set memberStatus

> _Note: this issue was identified against the code base at commit `67b7a4ea014b7cfd9cdb5213b26731b0db184c45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCollaboration

## Description
The emulator's Clean Rooms resource-creation handlers return HTTP 200 with a structurally incomplete resource: optional members of Collaboration/Membership that AWS populates (from the request or from a server-side default) are omitted from the serialized body entirely, and the creator's member status is reported with the wrong enum value. Two causes, both in the response construction for the create operations.

Cause 1 of 2 — CreateCollaboration omits analyticsEngine/isMetricsEnabled and returns memberStatus=ACTIVE instead of INVITED (fixture cleanrooms-change-request-cancel-workflow, step setup#1).
AWS: Smithy: CreateCollaboration -> CreateCollaborationOutput.collaboration (required, shape Collaboration), HTTP 200 POST /collaborations. Collaboration.analyticsEngine targets the AnalyticsEngine enum, Collaboration.isMetricsEnabled targets Boolean, and Collaboration.memberStatus is @required targeting MemberStatus with values [INVITED, ACTIVE, LEFT, REMOVED]. CreateCollaborationInput carries both analyticsEngine and isMetricsEnabled, so the created collaboration must reflect what the caller asked for. For memberStatus the model states only the enum domain; the recording shows the creator's status is INVITED in the CreateCollaboration response (the creator's own membership does not exist until the separate CreateMembership call later in this same fixture). Hypothesis: the creator becomes ACTIVE only once CreateMembership succeeds, so a freshly created collaboration always reports INVITED for the caller.
Emulator: replay of setup#1 returns 200 but $.collaboration.analyticsEngine is missing (expected 'SPARK', got None), $.collaboration.isMetricsEnabled is missing (expected false, got None), and $.collaboration.memberStatus is 'ACTIVE' where 'INVITED' is expected.
Why: a client that selects an analytics engine or toggles metrics at creation time cannot read back what it created — the member is absent, not false/null — so SDK accessors return unset and configuration-verification logic concludes the request was ignored. A client that polls memberStatus to learn whether it still has to create its own membership sees ACTIVE immediately and skips CreateMembership, so the creator never joins the collaboration it owns.
Fix: the CreateCollaboration handler's response construction — echo analyticsEngine and isMetricsEnabled from the input into the stored collaboration and serialize them, and derive memberStatus from whether the caller has an existing membership on the collaboration rather than hard-coding ACTIVE.

Cause 2 of 2 — CreateMembership omits isMetricsEnabled and the whole paymentConfiguration.machineLearning sub-structure (fixture cleanrooms-change-request-cancel-workflow, step setup#2).
AWS: Smithy: CreateMembership -> CreateMembershipOutput.membership (required, shape Membership), HTTP 200 POST /memberships. Membership.isMetricsEnabled targets Boolean and CreateMembershipInput accepts isMetricsEnabled. Membership.paymentConfiguration is @required targeting MembershipPaymentConfiguration, whose machineLearning member targets MembershipMLPaymentConfig with members modelTraining, modelInference and syntheticDataGeneration (each an isResponsible payment config). The recording shows the service materializing machineLearning with all three sub-configs present and isResponsible=false when the caller does not specify them, i.e. the ML payment block is defaulted server-side rather than left absent.
Emulator: replay of setup#2 returns 200 but $.membership.isMetricsEnabled is missing (expected false, got None) and $.membership.paymentConfiguration.machineLearning is missing entirely (expected {'modelInference': {'isResponsible': false}, 'modelTraining': {'isResponsible': false}, 'syntheticDataGeneration': {'isResponsible': false}}, got None).
Why: a client reading the payment responsibilities it accepted finds no machineLearning block at all, so it cannot tell whether ML costs are unowned or merely unreported, and code that walks paymentConfiguration.machineLearning.modelTraining.isResponsible dereferences an absent structure instead of reading false. The missing isMetricsEnabled has the same effect as in cause 1: the membership's metrics setting is unreadable after creation.
Fix: the CreateMembership handler's response construction / membership state — populate isMetricsEnabled from the input and materialize the defaulted MembershipMLPaymentConfig (all three sub-configs with isResponsible=false) inside MembershipPaymentConfiguration when the request omits it, then serialize both.

No open issue covers either cause; this is the first cleanrooms filing. Both causes were observed with state_reset_failures=0, so neither is contaminated by a previous fixture's state.

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, steps_total=14, passed=3, mismatched=9, blocked=2, state_reset_failures=0, score_pct=21.4.

Cause 1 — cleanrooms CreateCollaboration, fixture cleanrooms-change-request-cancel-workflow step setup#1, verdict=mismatch, expected_status=200, actual_status=200:
  $.collaboration.analyticsEngine: missing (expected 'SPARK', got None)
  $.collaboration.isMetricsEnabled: missing (expected False, got None)
  $.collaboration.memberStatus: value_mismatch (expected 'INVITED', got 'ACTIVE')
Model used: Collaboration.memberStatus @required -> MemberStatus [INVITED, ACTIVE, LEFT, REMOVED]; Collaboration.analyticsEngine -> AnalyticsEngine enum; Collaboration.isMetricsEnabled -> Boolean; CreateCollaborationInput.{analyticsEngine,isMetricsEnabled}.

Cause 2 — cleanrooms CreateMembership, fixture cleanrooms-change-request-cancel-workflow step setup#2, verdict=mismatch, expected_status=200, actual_status=200:
  $.membership.isMetricsEnabled: missing (expected False, got None)
  $.membership.paymentConfiguration.machineLearning: missing (expected {'modelInference': {'isResponsible': False}, 'modelTraining': {'isResponsible': False}, 'syntheticDataGeneration': {'isResponsible': False}}, got None)
Model used: Membership.paymentConfiguration @required -> MembershipPaymentConfiguration{queryCompute @required, machineLearning -> MembershipMLPaymentConfig{modelTraining, modelInference, syntheticDataGeneration}, jobCompute}; Membership.isMetricsEnabled -> Boolean; CreateMembershipInput.isMetricsEnabled.

Status codes match in both cases (200/200): the divergence is purely in the response body shape and in the memberStatus enum value.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay wire fixture 'cleanrooms-change-request-cancel-workflow' from trace set simparity-wire-aws-v1.
3. Cause 1: at step 'setup#1', POST /collaborations (CreateCollaboration) with analyticsEngine and isMetricsEnabled set. The emulator answers 200; compare the body against the recording — $.collaboration.analyticsEngine and $.collaboration.isMetricsEnabled are absent and $.collaboration.memberStatus is ACTIVE where the recording has INVITED.
4. Cause 2: at step 'setup#2', POST /memberships (CreateMembership) for that collaboration with isMetricsEnabled set and no explicit ML payment configuration. The emulator answers 200; $.membership.isMetricsEnabled is absent and $.membership.paymentConfiguration.machineLearning is absent instead of the defaulted modelTraining/modelInference/syntheticDataGeneration block with isResponsible=false.
5. Re-replay both steps (cleanrooms-change-request-cancel-workflow#setup#1 and #setup#2) to confirm a fix; both must pass.
