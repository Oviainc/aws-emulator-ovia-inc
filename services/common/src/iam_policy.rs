// SPDX-License-Identifier: CC0-1.0

//! EMPTY LEFTOVER — nothing here. This file is not declared in
//! `services/common/src/lib.rs` and is not compiled.
//!
//! It held a draft extraction of the IAM JSON resource-based policy grammar out of
//! `service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/state.rs`. That
//! extraction was WITHDRAWN from the fix it travelled with (a cloudtrail
//! `PutResourcePolicy` stub, which does not need it): ECR keeps its own grammar, exactly
//! as mainline has it. The draft content is removed rather than left dormant, so no
//! second, uncompiled copy of that grammar can drift from the live one.
//!
//! The file itself remains only because the fixer tooling cannot delete a file — the same
//! reason the cloudtrail crate still declares an empty `tests/all/wire.rs`. A follow-up
//! that owns `services/common` can write the shared grammar here, declare it, and migrate
//! ECR, `oam`, `ecrpublic` and `rum` onto it; cloudtrail's own `src/state.rs` carries a
//! MIGRATION NOTE asking for exactly that.
