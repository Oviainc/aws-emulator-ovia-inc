Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: healthlake.concurrent-publish-profile-versions-sequential healthlake.data-transformation-profile-draft-update-persists healthlake.delete-profile-removes-all-published-versions healthlake.get-profile-out-of-range-version-not-found healthlake.publish-from-existing-version-rollback healthlake.publish-increments-version-and-freezes-snapshot healthlake.publish-profile-exceeds-version-quota-rejected healthlake.start-transformation-job-nonexistent-profile-not-found healthlake.start-transformation-job-untrusted-role-rejected healthlake.tag-and-untag-profile-roundtrip healthlake.transform-data-sync-csv-to-fhir-bundle healthlake.update-profile-mapping-exceeds-entry-limit-rejected
Issue-Shape-Id: com.amazonaws.healthlake#HealthLake
Issue-Key: healthlake|CreateDataTransformationProfile|NotImplemented
Issue-Commit: 6136e2766a973f12a1067e5538f7a2dced441244

# Implement healthlake CreateDataTransformationProfile: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6136e2766a973f12a1067e5538f7a2dced441244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataTransformationProfile

## Description
`CreateDataTransformationProfile` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-healthlake-healthlake/src/ops/create_data_transformation_profile.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

12 recorded unit(s) fail on their `CreateDataTransformationProfile` call, so each one unblocks only when `CreateDataTransformationProfile` works: `healthlake.concurrent-publish-profile-versions-sequential`, `healthlake.data-transformation-profile-draft-update-persists`, `healthlake.delete-profile-removes-all-published-versions`, `healthlake.get-profile-out-of-range-version-not-found`, `healthlake.publish-from-existing-version-rollback`, `healthlake.publish-increments-version-and-freezes-snapshot`, `healthlake.publish-profile-exceeds-version-quota-rejected`, `healthlake.start-transformation-job-nonexistent-profile-not-found`, `healthlake.start-transformation-job-untrusted-role-rejected`, `healthlake.tag-and-untag-profile-roundtrip`, `healthlake.transform-data-sync-csv-to-fhir-bundle`, `healthlake.update-profile-mapping-exceeds-entry-limit-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-healthlake-healthlake/src/ops/create_data_transformation_profile.rs`. Replaying the corpus, the emulator refused the `CreateDataTransformationProfile` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDataTransformationProfile operation: operation CreateDataTransformationProfile is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the healthlake emulator
Call CreateDataTransformationProfile and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
