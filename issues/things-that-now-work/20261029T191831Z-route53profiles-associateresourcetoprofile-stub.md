Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53profiles.associate-resource-firewall-rule-group-lifecycle route53profiles.disassociate-resource-from-profile-removes-it route53profiles.list-profile-resource-associations-filter-by-type route53profiles.update-profile-resource-association-name
Issue-Shape-Id: com.amazonaws.route53profiles#Route53Profiles
Issue-Key: route53profiles|AssociateResourceToProfile|NotImplemented
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# Implement route53profiles AssociateResourceToProfile: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateResourceToProfile

## Description
`AssociateResourceToProfile` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-route53profiles-route53profiles/src/ops/associate_resource_to_profile.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `AssociateResourceToProfile` call, so each one unblocks only when `AssociateResourceToProfile` works: `route53profiles.associate-resource-firewall-rule-group-lifecycle`, `route53profiles.disassociate-resource-from-profile-removes-it`, `route53profiles.list-profile-resource-associations-filter-by-type`, `route53profiles.update-profile-resource-association-name`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-route53profiles-route53profiles/src/ops/associate_resource_to_profile.rs`. Replaying the corpus, the emulator refused the `AssociateResourceToProfile` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AssociateResourceToProfile operation: operation AssociateResourceToProfile is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the route53profiles emulator
Call AssociateResourceToProfile and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
