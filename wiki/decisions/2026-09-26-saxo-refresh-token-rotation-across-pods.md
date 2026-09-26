---
type: decision
tags:
  - daytrader/wiki
  - saxo
  - safety
updated: 2026-09-26
sources:
  - src/state.rs
  - src/auth.rs
  - src/shutdown.rs
---

# Saxo refresh-token rotation across pods

On 2026-09-26 the Saxo SIM refresh token was refused (HTTP 401) 46 minutes
before it expired, and the scheduler failed every cycle until the operator
logged in again at 06:39Z. This record keeps what the evidence shows apart
from what it cannot show, and the invariants the fix establishes.

## What the evidence shows

All from non-secret metadata: `saxo_sessions` lifecycle keys,
`scheduler_cycle_history`, and ReplicaSet creation times.

- The token was last rotated at 04:42:29Z by the scheduler's second cycle after
  the 04:31 release. The scheduler cycle at 04:56:00Z presented that token and
  Saxo refused it: the cycle's own `saxo_session` step carries the 401.
- That step took **5,891 ms**. Every successful refresh in the 250 retained
  cycles (from 2026-09-24T11:13Z) took at most 449 ms. The extra time
  matches the refresh-lease wait loop (1-second polls), meaning another process
  held the refresh lease while the scheduler waited.
- The scheduler's new ReplicaSet appeared exactly 30 s after the API's in all
  three releases that day (04:31:19→04:31:49, 04:44:50→04:45:20,
  06:18:51→06:19:21). `Recreate` waits for the old pod to die, so the old
  scheduler ignored SIGTERM and died only to SIGKILL. Every mode runs
  `/app/saxo-rust` as PID 1 and handled only SIGINT, and PID 1 gets no default
  action for SIGTERM.
- No refresh was due while either morning release was terminating pods. The
  access tokens expired at 04:50:44Z and 05:02:29Z, so with the 15-minute
  margin they became due at 04:35:44Z and 04:47:29Z. The old pods were gone
  by about 04:32:30Z and 04:46Z. **The releases did not strand the token by
  killing a pod mid-rotation that morning**, although the code allowed it.

## What the evidence cannot show

- **Which process held the lease at 04:56.** The pod logs are gone. The
  durable row cannot say either: on a refusal, the code at the time wrote the
  refusing pod's *own working copy* over the durable row unconditionally. The
  row's `last_refreshed_at` 04:42:29Z is therefore the scheduler's stale copy,
  not evidence that nobody rotated the token after 04:42.
- **Whether Saxo also revoked the token on its own.** Nothing seen excludes it.

## Defects found

Each is reproduced by a test in `src/state.rs`. The first three fail on the
pre-fix code and pass after it; the fourth reproduces a failure no
in-process code can survive.

1. **Lease gap.** A pod decided to refresh from its working copy, then took the
   lease. If another pod rotated the token and released the lease in between,
   the first pod presented the consumed token.
   (`a_pod_that_wins_the_lease_after_another_rotated_presents_the_durable_token`)
2. **Refusal clobber.** On a 401 the refusing pod wrote its stale session,
   marked invalid, over whatever the durable row held, including a newer
   valid rotation.
   (`a_refusal_for_a_superseded_token_leaves_the_newer_durable_session_live`)
3. **Stale file beats the durable row.** The restore rule ranked any
   refreshable-looking working copy above a refused durable row, even an older
   consumed token, which then got presented again.
   (`a_stale_working_copy_never_displaces_a_newer_durable_session`)
4. **SIGKILL mid-rotation.** A pod killed after Saxo rotated the token but
   before the rotation was durable leaves the durable row holding a consumed
   token, and its replacement is refused.
   (`a_pod_killed_between_rotation_and_the_durable_write_strands_its_replacement`)
5. A failed durable write after a successful rotation skipped the lease release
   and left the only live token in the pod's `/tmp`. The OAuth callback
   persisted by re-reading the file, which a concurrent restore could have
   just rewritten with the replaced session.

## Invariants now

- **Only the durable row is ever presented to Saxo, and only under the lease.**
  `AppState::rotate_saxo_session_under_lease` re-reads the row after taking
  the lease. The file-based refresh paths (`auth::ensure_session_json`,
  `auth::refresh_session`, `auth_status(auto_refresh)`) are removed, so no code
  path can refresh from the working copy.
- **A refusal is recorded only if the row still holds the refused token**, by
  compare-and-set on the stored text. Otherwise the newer row is adopted.
- **Restore rule:** between different refresh tokens the more recent session
  wins, because rotation is linear and a login is newer than any rotation. For
  the same token, a recorded refusal wins.
- **A rotation is durable before the lease is released.** The write is retried;
  if it still fails, the lease is kept until it lapses, so no other pod
  presents the consumed token in the meantime.
- **SIGTERM drains** ([src/shutdown.rs](/Users/lindau/codex/rust_daytrader/src/shutdown.rs)). A process stops starting
  rotations at once and waits up to 20 s for one in flight to become durable.
  While draining it still serves an access token that outlives 60 s. API and
  MCP keep accepting for 5 s so the Service stops routing to them first. The
  scheduler finishes its current cycle and starts no new one.
- One rotation per process at a time (single-flight). Concurrent requests share
  it.

## Behaviour changes worth knowing

- API and MCP pods now exit about 5 s after SIGTERM instead of being SIGKILLed
  at 30 s, and a scheduler `Recreate` no longer costs a fixed 30 s.
- `saxo_sessions.updated_at`/`source` now change only when a rotation, refusal
  or login is written. Before, every successful use rewrote the row with the
  caller's source, so older `source` values name the last reader, not the last
  rotation.

## If it recurs

Check the scheduler cycle's `saxo_session` error and step duration first. A
duration of several seconds before a 401 means another process was rotating
the token at the same time. `refresh_lease_source` on the row names the
current holder while a lease is held. Re-authentication stays the operator's
action.
