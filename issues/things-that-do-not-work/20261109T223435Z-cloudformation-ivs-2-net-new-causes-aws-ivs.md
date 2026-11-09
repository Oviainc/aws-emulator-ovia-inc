Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ivs.deploy-ivs-channel-stream-key-playback-key-pair-and-restriction-policy-round-trip-and-delete cloudformation__ivs.deploy-ivs-playback-key-pair-invalid-public-key-material-rejected-and-rolled-back
Issue-Key: cloudformation__ivs|CreateStack|ValidationError
Issue-Key: cloudformation__ivs.deploy-ivs-playback-key-pair-invalid-public-key-material-rejected-and-rolled-back|run#8|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__ivs: 2 net-new causes — AWS::IVS::PlaybackRestrictionPolicy unregistered, and invalid playback-key material is accepted instead of rolling the stack back

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two distinct root causes. Each is numbered with its own failing check id; both must be closed before this issue is done.

Cause 1 of 2: AWS::IVS::PlaybackRestrictionPolicy is not a recognized resource type.
AWS: it is a public CloudFormation resource type, so CreateStack accepts a template that declares it alongside the channel, stream key and playback key pair.
Emulator: CreateStack answers HTTP 400 `ValidationError` with `Template format error: Unrecognized resource types: [AWS::IVS::PlaybackRestrictionPolicy]`.
Why: an IVS channel estate that includes a playback restriction policy cannot be deployed at all — the whole stack is refused, not just that resource.
Fix: register AWS::IVS::PlaybackRestrictionPolicy in the CloudFormation resource-type registry and add its resource handler.
Traces: cloudformation__ivs.deploy-ivs-channel-stream-key-playback-key-pair-and-restriction-policy-round-trip-and-delete.

Cause 2 of 2: invalid public key material on AWS::IVS::PlaybackKeyPair is accepted and the stack reaches CREATE_COMPLETE.
AWS: ImportPlaybackKeyPair refuses PublicKeyMaterial that is not a valid PEM-encoded ECDSA public key with a ValidationException, so CloudFormation marks the resource CREATE_FAILED and the stack ROLLBACK_COMPLETE.
Emulator: the stack ends `CREATE_COMPLETE`.
Why: a client believes it imported a usable playback key pair; playback signing then fails at runtime with no deploy-time signal.
Fix: the AWS::IVS::PlaybackKeyPair resource handler's key-material validation / propagation of the IVS refusal.
Traces: cloudformation__ivs.deploy-ivs-playback-key-pair-invalid-public-key-material-rejected-and-rolled-back.

## Evidence
Cause 1: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::IVS::PlaybackRestrictionPolicy].
Cause 2: AssertionError: rejected stack ended 'CREATE_COMPLETE' — expected ROLLBACK_COMPLETE.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. Cause 1: CreateStack with a template declaring AWS::IVS::PlaybackRestrictionPolicy.
3. Cause 2: CreateStack with an AWS::IVS::PlaybackKeyPair whose PublicKeyMaterial is not a valid PEM ECDSA key, and watch the final stack status.
