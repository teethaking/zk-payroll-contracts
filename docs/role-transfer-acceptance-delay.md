# Role Transfer Acceptance Delay (#507)

A pending recipient must explicitly accept a privileged role transfer, and the
transfer cannot become active until the acceptance delay has elapsed. This
documents the on-chain behavior in `contracts/payroll/src/lib.rs`, the
privacy-safe status views it adds, and the SDK/dashboard workflow around it.

## Motivation

The payroll contract already made privileged role transfers two-step:

| Role | Propose | Accept | Cancel |
| --- | --- | --- | --- |
| Company admin | `propose_admin_rotation` | `accept_admin_rotation` | `cancel_admin_rotation` |
| Treasury owner | `propose_treasury_rotation` | `accept_treasury_rotation` | `cancel_treasury_rotation` |
| Admin handover (#339) | `request_admin_handover` | `accept_admin_handover` | `cancel_admin_handover` |

Neither side can complete a transfer alone — but the two steps could happen in
the same ledger. A proposer whose key is compromised (or an operator acting on
a forged proposal) could hand a privileged role to an attacker and have it
activated immediately, before the outgoing holder or an off-chain monitor had
any chance to notice and cancel.

#507 inserts a mandatory waiting period between the two steps.

## Acceptance delay

`ROLE_TRANSFER_ACCEPTANCE_DELAY` (24 hours, in seconds) is the minimum time
that must pass between a proposal's ledger timestamp and the recipient's
acceptance. It applies to **all three** privileged transfers in the contract.

| Operation | Who | Effect |
| --------- | --- | ------ |
| `propose_admin_rotation(current_admin, new_admin)` | Current admin | Stores the proposal and its `proposed_at` timestamp. Unchanged. |
| `accept_admin_rotation(new_admin)` | Proposed admin | Rejects while `now < proposed_at + ROLE_TRANSFER_ACCEPTANCE_DELAY`. |
| `propose_treasury_rotation(current_owner, new_owner)` | Treasury owner | Stores the proposal and its `proposed_at` timestamp. Unchanged. |
| `accept_treasury_rotation(new_owner)` | Proposed owner | Rejects while `now < proposed_at + ROLE_TRANSFER_ACCEPTANCE_DELAY`. |
| `request_admin_handover(current_admin, pending_admin)` | Current admin | Stores the request and its `requested_at` timestamp. Unchanged. |
| `accept_admin_handover(pending_admin)` | Pending admin | Rejects while `now < requested_at + ROLE_TRANSFER_ACCEPTANCE_DELAY`. |
| `get_admin_transfer_status()` | Anyone | `Some(RoleTransferStatus)` while an admin transfer is pending, else `None`. |
| `get_treasury_transfer_status()` | Anyone | `Some(RoleTransferStatus)` while a treasury transfer is pending, else `None`. |
| `get_handover_transfer_status()` | Anyone | `Some(RoleTransferStatus)` while a handover is pending, else `None`. |

Nothing about payroll execution changes: preparing, approving, finalizing, and
executing payroll runs are untouched. Only the *activation* of a transferred
privileged role is gated.

## Scope

This covers the privileged role transfers that control the payroll contract
itself (admin, treasury owner, admin handover). Two-step role changes in other
contracts — the `pause_manager` operator rotation and the `payroll_registry`
company admin/treasury rotations — are unchanged and still accept immediately
after a proposal. Extending the same gate there is a follow-up that needs its
own test and SDK pass.

## Lifecycle

```
propose_*_rotation / request_admin_handover
        │
        │  acceptance delay elapses (24h)
        ▼
accept_*  ── before the window ──► rejected (proposal stays pending)
        │
        ▼
   role active  (and the #253 configuration lock still applies)
```

1. **Inside the window** — acceptance is rejected; the proposal stays pending
   and the outgoing holder keeps the role.
2. **During the window** — the proposer may still cancel at any time
   (`cancel_admin_rotation`, `cancel_treasury_rotation`,
   `cancel_admin_handover`). Cancellation is never blocked, so an unwanted
   transfer can never park a role indefinitely.
3. **At or after `ready_at`** — the recipient accepts and the role becomes
   active, subject to the pre-existing locks: acceptance is still rejected
   while a payroll run is prepared but unresolved (#253).

## Check ordering (why it matters)

Each accept entry-point runs its checks in this order:

1. pause gate (`require_not_paused`)
2. proposal must exist
3. **caller must be the proposed recipient** (authorization)
4. **acceptance delay elapsed** (#507)
5. no payroll run pending (#253)
6. role is written, proposal cleared, `*_rotated` / `*_accepted` event emitted

The recipient check deliberately precedes the delay check. Otherwise an
arbitrary caller could learn *when* a transfer becomes acceptable for a role
they have no claim to, turning the delay into a status oracle for privileged
role movements.

## Failure behavior

- **Too early** — panics with
  `"Role transfer acceptance delayed: <role> transfer becomes acceptable in <n>s"`,
  where `<role>` is `admin`, `treasury_owner`, or `admin_handover` and `<n>` is
  the remaining seconds. The message is actionable (the client knows exactly
  how long to wait) and privacy-safe: it contains timing information only, no
  salaries, commitments, employee addresses, or run amounts.
- **Not the recipient** — unchanged: panics with the pre-existing
  `"Unauthorized: caller is not the proposed admin"` /
  `"... proposed treasury owner"` / `"... pending admin"` messages.
- **Run pending (#253)** — unchanged configuration-lock message.
- **Cancelled transfer** — the pending record is gone, so acceptance panics
  with the pre-existing "No pending ..." message.

The delay itself is a contract constant rather than stored policy: `DataKey`
already sits at the 50-variant ceiling of the Soroban contract-spec UDT union
(`ScSpecUdtUnionV0.cases` is a `VecM<_, 50>`), so adding a storage key for a
configurable delay fails contract generation. Operators who need a different
window must ship a patched build; because cancellation is always available,
no window length makes a transfer irreversible.

## Tests

- `contracts/payroll/src/lib.rs` (unit tests) — `test_role_transfer_acceptance_delay_blocks_early_accept`,
  `test_role_transfer_delay_does_not_bypass_authorization`,
  `test_role_transfer_cancellable_during_delay`,
  `test_treasury_and_handover_transfers_honor_delay`, plus the updated
  rotation/handover flow tests.
- `tests/access-control/role_transfer_delay.rs` (`access_control_tests`) —
  end-to-end coverage through the generated client: delayed activation, the
  treasury and handover paths, and cancellation inside the window.

```bash
cargo test -p payroll --lib
cargo test -p access_control_tests --test role_transfer_delay
```

## SDK / dashboard guidance

- After proposing, read the matching status view and show a countdown to
  `ready_at`. Do not poll for an applied role change — it will not happen
  before the window opens.
- Treat an early-accept failure as retryable *later*, not as a terminal error:
  surface the remaining seconds from the message (or recompute from
  `ready_at`) instead of showing a generic failure.
- Never treat the delay as a substitute for recipient verification: only the
  address recorded in the proposal can ever activate the role.
