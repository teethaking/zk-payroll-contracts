# PR Description

## Title
`feat(payroll): require an acceptance delay before a privileged role transfer activates (#507)`

## Description

Privileged role transfers in the payroll contract were already two-step (propose
→ accept), but both steps could land in the same ledger. A proposer whose key
was compromised could hand the admin or treasury-owner role to an attacker and
have it activated immediately, before the outgoing holder or an off-chain
monitor had a chance to cancel.

This PR adds a mandatory 24-hour acceptance delay: a pending recipient can only
activate a transferred role once the window since the proposal has elapsed.

### Key Changes

- **`contracts/payroll/src/lib.rs`**
  - Adds `ROLE_TRANSFER_ACCEPTANCE_DELAY` (24h) and the privacy-safe
    `RoleTransferStatus` view (`role`, `new_holder`, `proposed_at`, `ready_at`,
    `delay_seconds`).
  - Enforces the delay in `accept_admin_rotation`, `accept_treasury_rotation`,
    and `accept_admin_handover` — **after** the recipient identity check and
    **before** the existing #253 configuration lock, so an unauthorized caller
    cannot use the gate as a privileged-transfer status oracle.
  - Adds read-only views `get_admin_transfer_status()`,
    `get_treasury_transfer_status()`, and `get_handover_transfer_status()` so
    dashboards can render a countdown without reading private payroll data.
  - Failure message is timing-only and actionable:
    `"Role transfer acceptance delayed: <role> transfer becomes acceptable in <n>s"`.
- **`tests/access-control/role_transfer_delay.rs`** (new target): end-to-end
  coverage of the delayed activation, the treasury/handover paths, and
  cancellation inside the window.
- **Docs**: `docs/role-transfer-acceptance-delay.md` (new), plus updates to
  `docs/errors.md` (recovery guidance for the new failure) and `README.md`.

### Notes for reviewers

- Payroll execution is untouched: preparing, approving, finalizing, and
  executing runs behave exactly as before.
- A pending transfer stays fully cancellable for the whole window, so no delay
  length can park a role indefinitely.
- The delay is a contract constant, not stored policy: `DataKey` already sits at
  the 50-variant ceiling of the Soroban contract-spec UDT union
  (`ScSpecUdtUnionV0.cases` is a `VecM<_, 50>`), so adding a storage key for a
  configurable delay fails contract generation. Making it configurable needs a
  storage-key refactor first.

## How Was This Tested?

- Unit tests in `contracts/payroll/src/lib.rs`:
  - `test_role_transfer_acceptance_delay_blocks_early_accept` — main path:
    accept is rejected one second before `ready_at`, succeeds exactly at
    `ready_at`, and the proposal survives a rejected attempt.
  - `test_role_transfer_delay_does_not_bypass_authorization` — edge case: an
    impostor is still rejected after the window opens, and the rightful
    recipient still completes the transfer.
  - `test_role_transfer_cancellable_during_delay` — edge case: cancellation
    inside the window, and the cancelled proposal stays dead afterwards.
  - `test_treasury_and_handover_transfers_honor_delay` — treasury rotation and
    admin handover honour the same delay.
  - `test_accept_admin_rotation_rejects_wrong_address` now advances the clock
    past the delay, pinning the check ordering.
- Integration tests: `tests/access-control/role_transfer_delay.rs` and the
  updated `tests/access-control/admin_handover.rs`, `tests/admin/`, and
  `tests/migrations/` role-transfer flows.

```bash
cargo fmt --all --check
cargo clippy -p payroll --all-targets -- -D warnings
cargo test -p payroll --lib
cargo test -p access_control_tests --test role_transfer_delay
cargo test -p admin_config_lock_tests
cargo test -p migration_tests
```

> Note: the workspace has pre-existing build failures unrelated to this change
> (`contracts/audit_module` calls a missing `payroll_events::emit_audit_grant_pruned`,
> `contracts/payroll/tests/overpayment_review.rs` has a `ufn` typo, and
> `tests/access-control/authorization_matrix.rs` does not compile). They exist on
> `main` and are untouched here.

## Closes
Closes #507
