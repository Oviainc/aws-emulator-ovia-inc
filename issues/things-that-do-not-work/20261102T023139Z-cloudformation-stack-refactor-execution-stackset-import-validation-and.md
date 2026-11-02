Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-stack-refactor-lifecycle
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-stack-refactor-lifecycle cloudformation-stackset-instance-management-errors cloudformation-type-registry-reads
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|DescribeStackRefactor|2xx->2xx|$
Issue-Key: cloudformation|DescribeStackRefactor|2xx->2xx|$.DescribeStackRefactorResponse.DescribeStackRefactorResult.ExecutionStatus
Issue-Key: cloudformation|ImportStacksToStackSet|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudformation|ImportStacksToStackSet|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|ImportStacksToStackSet|4xx->4xx|status
Issue-Key: cloudformation|ListTypes|2xx->2xx|$.ListTypesResponse.ListTypesResult.NextToken
Issue-Key: cloudformation|ListTypes|2xx->2xx|$.ListTypesResponse.ListTypesResult.TypeSummaries.member[*]
Issue-Commit: f53494b98f8fcfae7b43729e0559b63c793ee9fd

# cloudformation stack-refactor execution, StackSet import validation and type registry listing diverge

> _Note: this issue was identified against the code base at commit `f53494b98f8fcfae7b43729e0559b63c793ee9fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackRefactor

## Description
Three net-new wire divergences in the CloudFormation control plane, all on surfaces that report the state of an asynchronous or registry-backed object. Each cause is independent and must be closed separately.

=== Cause 1 of 3: ExecuteStackRefactor never advances the refactor's ExecutionStatus (fixture cloudformation-stack-refactor-lifecycle, step task#12, operation DescribeStackRefactor) ===
AWS: Smithy: ExecuteStackRefactor(StackRefactorId) -> Unit; DescribeStackRefactor output member ExecutionStatus targets enum StackRefactorExecutionStatus with values [UNAVAILABLE, AVAILABLE, OBSOLETE, EXECUTE_IN_PROGRESS, EXECUTE_COMPLETE, EXECUTE_FAILED, ROLLBACK_IN_PROGRESS, ROLLBACK_COMPLETE, ROLLBACK_FAILED], and Status targets StackRefactorStatus. A refactor that has been created is AVAILABLE; once ExecuteStackRefactor is accepted the refactor leaves AVAILABLE and settles on a terminal execution status (EXECUTE_COMPLETE on success, EXECUTE_FAILED/ROLLBACK_* otherwise). Recorded behavior: the step polls DescribeStackRefactor until ExecutionStatus is EXECUTE_COMPLETE and AWS reaches it inside the poll budget.
Emulator: DescribeStackRefactor keeps answering 200 with Status 'CREATE_COMPLETE' and ExecutionStatus 'AVAILABLE' for the entire poll window — the replay reports "poll capped at 60s" and "ExecutionStatus: poll: not in ['EXECUTE_COMPLETE'] after 13 attempt(s) (got 'AVAILABLE')". The refactor never enters EXECUTE_IN_PROGRESS and never reaches any terminal execution status.
Why: every client that waits for a stack refactor to finish (the SDK/CLI waiters and any poll loop keyed on ExecutionStatus) hangs until its own deadline and can never observe the refactor's stack moves as applied. There is no status a caller can key on to decide success or failure.
Fix: the ExecuteStackRefactor handler and the stack-refactor state machine in the service's state — accepting the execute must transition the stored refactor's ExecutionStatus so DescribeStackRefactor reports progress and a terminal value.
Traces: cloudformation-stack-refactor-lifecycle#task#12. The open issue 20261029T214321Z-cloudformation-resource-scans-find-nothing-hook-handler-progress.md mentions DescribeStackRefactor among several behavioral root causes, but it does not carry this fixture/step replay key and does not cover the execute transition.

=== Cause 2 of 3: ImportStacksToStackSet does not validate the StackIds entries, so a malformed stack id is answered as a not-found instead of a validation error (fixture cloudformation-stackset-instance-management-errors, step task#5, operation ImportStacksToStackSet) ===
AWS: Smithy: ImportStacksToStackSet input StackIds targets StackIdList; the operation models both StackNotFoundException (httpError 404, awsQueryError code StackNotFoundException) and the query-protocol validation refusal. Recorded response: HTTP 400 with Error.Code 'ValidationError' and Error.Message '<stack-id> is not a valid StackId'. AWS checks the shape of each StackIds entry up front — a value that is not a stack id/ARN is a client validation failure, not a lookup miss. Message rule: the fixed text is ' is not a valid StackId'; the variable part is the offending StackIds entry copied verbatim from the request (the recorded run happened to use 'sim2real-probe-nonexistent-stack' — that value is the template slot, not part of the requirement).
Emulator: answers HTTP 404 with Error.Code 'StackNotFoundException' and Error.Message 'Stack with id <value> does not exist' — it skips the StackId format gate and goes straight to a stack lookup, then reports the miss.
Why: the status class and the error code both differ. SDK error mapping gives the caller a StackNotFoundException (a 404, commonly treated as a benign/eventually-consistent miss or retried) where AWS gives a non-retriable 400 ValidationError, so client code that branches on error class — import tooling that distinguishes 'you passed garbage' from 'that stack is gone' — takes the wrong branch, and the message text differs too.
Fix: ImportStacksToStackSet input validation, ahead of the stack lookup in its handler.
Traces: cloudformation-stackset-instance-management-errors#task#5. The open issue 20261029T214321Z-cloudformation-the-stack-not-found-refusal-message-is.md covers the 'Stack with id <name> does not exist' wording on five other operations; it does not cover this operation's missing StackId validation gate and does not carry this replay key.

=== Cause 3 of 3: ListTypes ignores the requested registry selection, omits most TypeSummary members and returns no NextToken (fixture cloudformation-type-registry-reads, step task#2, operation ListTypes) ===
AWS: Smithy: ListTypes input Visibility, ProvisioningType, DeprecatedStatus, Type (enum RegistryType: RESOURCE|MODULE|HOOK), Filters (TypeFilters: Category, PublisherId, TypeNamePrefix), MaxResults (range 1..100), NextToken (length 1..1024); output TypeSummaries + NextToken, @paginated(items=TypeSummaries, inputToken/outputToken=NextToken, pageSize=MaxResults). TypeSummary models Type, TypeName, DefaultVersionId, TypeArn, LastUpdated, Description, PublisherId, OriginalTypeName, PublicVersionNumber, LatestPublicVersion, PublisherIdentity, PublisherName, IsActivated. The recorded response's first TypeSummaries member is a HOOK extension — TypeName 'AWS::ControlTower::Hook', TypeArn of the form arn:aws:cloudformation:<region>::type/hook/<publisher>/<type>, carrying Description, IsActivated, PublisherIdentity, PublisherId, PublisherName, LatestPublicVersion, LastUpdated (and PublisherCategory 'VERIFIED_PUBLISHER', which this model version does not declare — recorded behavior only) — and the response carries a NextToken because the registry page is truncated.
Emulator: answers 200 whose first TypeSummaries member is a RESOURCE extension ('AWS::ACMPCA::Certificate', arn:...:type/resource/AWS-ACMPCA-Certificate) carrying only Type, TypeName and TypeArn, and omits NextToken entirely. Hypothesis: the request's registry selection (Type / Visibility / Filters) is not applied, so the handler returns a fixed resource-type catalogue, and the serializer populates none of the publisher/activation/description members.
Why: a client enumerating the registry gets extensions it did not ask for and none of the metadata it reads — publisher identity, activation state, available public version, description — so activation and hook-discovery flows see an empty or wrong registry. With NextToken absent on a truncated page the paginator stops after one page, so the rest of the registry is unreachable even when the filter happens to match.
Fix: the ListTypes handler (apply the request's Type/Visibility/DeprecatedStatus/Filters selection and emit NextToken when the page is truncated) and its TypeSummary serializer (emit the modelled members held in registry state).
Traces: cloudformation-type-registry-reads#task#2. Related open issues 20261028T094216Z-cloudformation-the-public-extension-registry-exposes-no-activatable.md and 20261029T214321Z-cloudformation-type-registry-testtype-s-publisher-gate-refuses.md touch the extension registry but neither covers this ListTypes response shape nor carries this replay key.

## Evidence
Cause 1 — cloudformation-stack-refactor-lifecycle#task#12, DescribeStackRefactor, status 200->200:
  expected: ExecutionStatus in ['EXECUTE_COMPLETE']
  actual:   {'DescribeStackRefactorResult': {'Description': 'probe-refactor', 'StackRefactorId': '<id>', 'StackIds': {'member': [<two stack ARNs>]}, 'ExecutionStatus': 'AVAILABLE', 'Status': 'CREATE_COMPLETE'}}
  replay detail: "poll capped at 60s"; "$.DescribeStackRefactorResponse.DescribeStackRefactorResult.ExecutionStatus: poll: not in ['EXECUTE_COMPLETE'] after 13 attempt(s) (got 'AVAILABLE')"

Cause 2 — cloudformation-stackset-instance-management-errors#task#5, ImportStacksToStackSet:
  expected: HTTP 400, $.ErrorResponse.Error.Code = 'ValidationError', $.ErrorResponse.Error.Message = '<StackIds entry> is not a valid StackId' (recorded: 'sim2real-probe-nonexistent-stack is not a valid StackId')
  actual:   HTTP 404, $.ErrorResponse.Error.Code = 'StackNotFoundException', $.ErrorResponse.Error.Message = 'Stack with id sim2real-probe-nonexistent-stack does not exist'

Cause 3 — cloudformation-type-registry-reads#task#2, ListTypes, status 200->200:
  expected $.ListTypesResponse.ListTypesResult.TypeSummaries.member[0] = {'Type': 'HOOK', 'TypeName': 'AWS::ControlTower::Hook', 'TypeArn': 'arn:aws:cloudformation:us-west-2::type/hook/aws-hooks/AWS-ControlTower-Hook', 'Description': 'AWS ControlTower Hook for Proactive Controls', 'IsActivated': 'false', 'PublisherIdentity': 'AWS', 'PublisherId': 'aws-hooks', 'PublisherName': 'aws-hooks', 'PublisherCategory': 'VERIFIED_PUBLISHER', 'LatestPublicVersion': '1.2.31', 'LastUpdated': <timestamp>}
  actual   $...TypeSummaries.member[0] = {'Type': 'RESOURCE', 'TypeName': 'AWS::ACMPCA::Certificate', 'TypeArn': 'arn:aws:cloudformation:us-west-2::type/resource/AWS-ACMPCA-Certificate'}
  expected $.ListTypesResponse.ListTypesResult.NextToken present; actual: missing (None)
(per-run values — ids, ARNs, timestamps, request ids — are environment-specific and are not themselves the requirement)

## Steps to Reproduce
Replay the wire corpus simparity-wire-aws-v1 against the emulator (replay_wire_corpus), or exercise the steps directly:

Cause 1: fixture cloudformation-stack-refactor-lifecycle, step task#12. Create two stacks, CreateStackRefactor over them, wait for Status CREATE_COMPLETE / ExecutionStatus AVAILABLE, call ExecuteStackRefactor(StackRefactorId), then poll DescribeStackRefactor for ExecutionStatus == EXECUTE_COMPLETE. The emulator stays at AVAILABLE for the full 60s poll (13 attempts).

Cause 2: fixture cloudformation-stackset-instance-management-errors, step task#5. Create a stack set, then call ImportStacksToStackSet(StackSetName=<that stack set>, StackIds=['sim2real-probe-nonexistent-stack']). Expect HTTP 400 ValidationError '<value> is not a valid StackId'; the emulator answers HTTP 404 StackNotFoundException 'Stack with id <value> does not exist'.

Cause 3: fixture cloudformation-type-registry-reads, step task#2. Call ListTypes as the fixture does and inspect TypeSummaries.member[0] and NextToken. The emulator returns a RESOURCE summary with only Type/TypeName/TypeArn and no NextToken.
