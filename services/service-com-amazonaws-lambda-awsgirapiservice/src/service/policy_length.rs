// SPDX-License-Identifier: CC0-1.0

//! WHERE this service answers for a resource-based policy that is too large — and why it
//! is not answered here.
//!
//! The published quota on a function's resource-based policy is 20 KB (Lambda quotas,
//! "Function resource-based policy"), and two different conditions reach it:
//!
//! * a GRANT that would push the policy a function already holds past that quota is the
//!   modelled `PolicyLengthExceededException` ("The permissions policy for the resource is
//!   too large"), answered by the `AddPermission` handler over
//!   [`crate::state::policy_length_exceeded_message`] — only a handler knows what the
//!   stored policy already holds, so only a handler can decide it;
//! * a `PutResourcePolicy` whose `Policy` member is itself longer than that member's
//!   modelled `@length` maximum is RECORDED as `ValidationException` — the constraint
//!   envelope every other broken constraint of this service is stated with (corpus
//!   `simparity-behavioral-aws-v1`, check `lambda.put-resource-policy-length-exceeded`,
//!   which asserts that error code) — so the framework's own boundary walk answers it and
//!   this layer adds nothing to it.
//!
//! This module therefore holds no code: it is the statement of that split, kept beside the
//! other seams [`crate::service`] owns so a later change does not move the first answer to
//! the boundary or re-word the second one as the operation's own error.
