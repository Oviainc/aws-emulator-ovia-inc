# lambda CreateCapacityProvider refuses an operator role absent from IAM (mainline gate red)

# Lambda CreateCapacityProvider: refuse an operator role this account's IAM holds no record of

> _Note: this issue was identified against the code base at commit `da870a15f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapacityProvider

## Description
`core/router/tests/all/lambda_capacity_provider_operator_role.rs` and the Lambda code disagree, so `cargo test -p aws-emulator-router --test all` fails on mainline and every fix's gate fails with it, whatever service the fix touches.

- The test row "a role this account's IAM holds none of" expects the create to be REFUSED (`accepted: false`). That is AWS's behavior; see Evidence.
- The code accepts it: `crate::state::capacity_provider::operator_role_is_unassumable` calls `service_common::iam_role::held_role_is_unassumable`, which answers `false` (do not refuse) for `RoleAnswer::Absent`.

STATUS UPDATE (absent-role cause closed): the code now refuses it —
`operator_role_is_unassumable` calls `service_common::iam_role::role_is_unassumable`, whose
`Absent` answer refuses — and the row "a role this account's IAM holds none of" expects that
refusal again, with a comment saying the expectation is AWS's recorded answer and must not be
flipped for another fix. `cargo test -p aws-emulator-router --test all` is green. WHAT REMAINS
is the cross-account cause below: a well-formed role ARN of ANOTHER account is still served,
where AWS refuses it with HTTP 403 `AccessDeniedException` and the message
`Cross-account pass role is not allowed.`, and that row still expects the create to be served.

Fix the CODE, not the test: make `operator_role_is_unassumable` refuse an operator role ARN of THIS account that IAM reports absent, with HTTP 400 `InvalidParameterValueException` and the fixed message `The operator role is invalid or doesn't have sufficient permissions. Verify the role and permissions and try again.`, persisting nothing. The test's row "a well-formed role ARN of another account" (`accepted: true`) is also wrong for AWS: AWS refuses it with HTTP 403 `AccessDeniedException`, message `Cross-account pass role is not allowed.`, persisting nothing. Make the code refuse it that way and change that row to expect the refusal with that code and message. An emulator with no IAM mounted still refuses nothing.

Do not change `held_role_is_unassumable` for its other callers (a function's execution role); answer the absent case inside the capacity-provider reading only.

Add a comment on `operator_role_is_unassumable` and on both test rows stating that these answers are AWS's recorded behavior for CreateCapacityProvider (the traces in Evidence), that they differ from the execution-role reading on purpose, and that a fix for another issue must not change them: if this test fails in a gate, the change under review broke it, and the change is what to fix.

Do not edit this test's expectations to get another fix through its gate. The row has flipped four times today (09:46, 11:53, 13:34, 13:54) in fixes for unrelated issues (Transfer, Ground Station), and each flip broke the gate for everyone else.

## Evidence
Real AWS refuses a nonexistent operator role in the caller's own account, in every recording:
- wire `lambda-capacity-provider-lifecycle` (scenario `capacity-provider-invalid-role-and-not-found-rejected`), step 2: `CreateCapacityProvider` with `PermissionsConfig.CapacityProviderOperatorRoleArn = arn:aws:iam::<account>:role/sim2real-probe-nonexistent-operator-role` answers 400, `x-amzn-errortype: InvalidParameterValueException`, body `{"Type": "User", "message": "The operator role is invalid or doesn't have sufficient permissions. Verify the role and permissions and try again."}`.
- A direct probe on real AWS (us-west-2, real subnet and security group): a nonexistent role of the caller's account is refused 400 with the message above (with fake subnet and security group ids too); a role of another account is refused 403 `AccessDeniedException` "Cross-account pass role is not allowed."; a role trusting only `ec2.amazonaws.com` is refused 400 with the message above; a role trusting `lambda.amazonaws.com` with `AWSLambdaManagedEC2ResourceOperator` is accepted 202, State `Pending`.
- behavioral `lambda.capacity-provider-invalid-params-rejected`: a never-created role name gets the same code and exact message, and `GetCapacityProvider` then answers `ResourceNotFoundException`.

## Steps to Reproduce
`cargo test -p aws-emulator-router --test all lambda_capacity_provider_operator_role` fails at the row "a role this account's IAM holds none of": "a role this service cannot assume must be refused", got 202.
