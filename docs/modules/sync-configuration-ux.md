# Cloud Sync Configuration UX Contract

## Status And Scope

This is a **binding requirement**, not a claim that the current implementation
already complies. The existing pause-on-expand and pause-before-browse paths
must be migrated. Implementation status and remaining gaps are recorded in
[`sync.md`](sync.md#edit-first-configuration-contract).

The governing rule is:

> 先编辑；只有确认并接受了真正的修改，才停止旧同步，再自动使用最新配置同步。

This applies to every exposed cloud-sync editing flow: individual upload-album
choices, Select All, local/remote root changes, connection or credential changes
where editable, task creation/deletion, and confirmed conflict choices. Future
sync configuration controls must use the same contract. Display-only preferences
and explicit opt-out/deletion have the exceptions below.

## Interaction Invariants

1. **Running is not read-only.** A running or internally quiescing job must not
   make its configuration controls unavailable. Users must not pause, resume,
   pull Photos, or wait for a transfer before they can edit or submit changes.
   Disable only the specific operation that would otherwise be duplicated or
   unsafe (for example, a second Delete confirmation for an already deleting
   task), not unrelated edits or the entire sync group.
2. **Viewing and drafting have no scheduling side effects.** Opening Settings,
   expanding an album checklist, browsing cloud folders, typing a draft,
   dismissing a picker, cancelling a confirmation, submitting invalid input, or
   submitting the same effective configuration must not pause/restart a job or
   advance its configuration generation.
3. **Compare effective values, not raw input.** Normalize and validate paths,
   album sets and other values before comparison. A different order or duplicate
   album name is not a change. Do not stop first and discover afterwards that
   the input was invalid or unchanged.
4. **Accepted changes drive reconfiguration.** Checkbox/switch changes are
   immediate-apply edits; forms and destructive choices require their explicit
   Save/Apply/confirmation. After a real edit is accepted, the application owns
   stopping the obsolete run, applying the latest configuration safely, and
   scheduling the next run. There is no subsequent manual resume/refresh step.
5. **Edits remain possible while changes are applying.** Additional changes
   update the latest desired configuration. A short debounce/coalescing window
   is allowed, but must not prevent input or require an Apply click for an
   immediate-apply control. A bulk Select All edit is one logical mutation, not
   one mutation/restart per album.
6. **Cloud presence is never upload permission.** Reconfiguration must preserve
   explicit local upload scope. Unchecked albums keep receiving cloud downloads;
   their existence in the cloud cannot opt them into uploads. Select All applies
   to the currently listed local albums, not future albums.

## Draft, Desired And Active Configuration

Keep these concepts distinct even if the implementation represents them with
existing fields, a pending-command record, or a separate configuration snapshot:

| Layer | Meaning | May affect the running engine? |
|---|---|---|
| Draft | Unconfirmed editor input | No |
| Desired | Latest validated, accepted configuration or mutation intent | Requests a safe transition |
| Active | Immutable configuration snapshot used by the current run | Only that run's operations may use it |

An accepted edit must have durable recovery evidence before stopping the old
run or performing destructive bookkeeping. This does **not** authorize writing
a new root into an in-flight operation's active context, deleting that operation's
entry records, or sweeping its recovery artifacts prematurely. Root/connection
changes may persist a pending desired mutation first and install the new active
configuration after the old run is quiescent.

The order is:

```text
running with active generation g
    ├─ open / browse / draft / cancel / invalid / no-op → keep running g
    └─ accept a real edit → record latest desired generation/intent
                              ↓
                         stop claiming old work
                              ↓
                         settle the in-flight operation safely
                              ↓
                         apply the latest accepted configuration
                              ├─ sync enabled + valid surviving job → reconcile and run latest
                              ├─ sync disabled → keep configuration, do not start network work
                              └─ confirmed deletion → remove job, never restart it
```

“Stop” here is an **internal transition after editing**, not an editing
prerequisite or a persisted user-facing pause requiring manual recovery. The
current file may need to finish atomic publication or reconciliation; editing
must remain available during that wait. Never abort an uncertain remote write
and assume it failed, discard the only complete snapshot, or roll back already
completed file changes merely because configuration changed.

## Serialization And Latest-Edit Wins

- Serialize configuration mutations and publishers per job. Reconfigure only
  affected jobs; ordinary edits must not stop unrelated jobs.
- Coalesce queued edits and trigger the latest accepted generation. Do not
  restart intermediate configurations for every checkbox notification.
- Merge independent edited fields against the latest desired state; an older
  album-selection result must not restore an old cloud root or credential.
- Once a real edit is accepted, the obsolete run must not claim another file
  under its old configuration. Work already issued retains its old generation
  and recovery context until its outcome is proven.
- Old workers, scans, recovery completions and UI callbacks must not overwrite
  newer configuration, revert newer controls, mark the latest generation fully
  synchronized, or clear a newer pending edit. Apply the generation guards from
  C06 in [`sync.md`](sync.md#design-constraints).
- Automatic restart must reload current jobs/configuration, not reuse a job list
  or snapshot captured before editing. Superseded, disabled, deleted or invalid
  jobs must not be resurrected by a remembered trigger.
- If the app exits during reconfiguration, startup must recover the accepted
  intent and safe file-operation evidence, then use the latest valid desired
  configuration. Never silently lose a saved edit because it was only queued
  in memory.

## Flow-Specific Rules

| Flow | What is side-effect free? | After an accepted effective change |
|---|---|---|
| Album checkbox / Select All | Expanding or inspecting the list | Persist one logical scope edit; internally stop obsolete work and automatically run the latest explicit scope |
| Cloud folder / root / connection / credentials | Opening a browser or typing an unconfirmed value | Validate and confirm; quiesce affected old work, safely install the new mapping and restart automatically |
| Add a task | Incomplete/invalid form and cancelled submission | Validate and save; schedule the new task if sync is enabled, without stopping unrelated jobs |
| Delete a task | Opening or cancelling its confirmation | Stop the confirmed target safely, remove its relationship, preserve local/cloud media; never restart the deleted job |
| Conflict choice | Inspecting conflicts or abandoning a choice | Re-check versions, serialize affected operations and apply the valid choice; reconcile the resulting latest state automatically |
| Global sync off | Merely inspecting the switch | Persist explicit opt-out and stop scheduling; never automatically turn it back on or restart network work |
| Global sync on | Redundant assignment of the current value | Read the latest saved configurations and automatically schedule eligible jobs |
| Day-grid cloud badges / display-only settings | Both inspection and changes | Update presentation only; never stop or restart transfers |

Explicit disabling, deletion, invalid configuration, loss of credentials/root
reachability, or a safety block can prevent a new run. Report the actual reason;
these exceptions must not reintroduce “pause before editing.” A credential error
must not expose secrets or roll back a newer unrelated configuration choice.

## Feedback And Failure Handling

- Reflect accepted album choices, counts, master-switch state and checked-first
  ordering immediately; show a bounded “Applying changes” / “Switching to latest
  configuration” status while the worker transitions.
- Do not replace the editable interface with a waiting screen or present
  internal quiescence as a prerequisite the user must resolve.
- Validation failures and no-ops leave the existing run untouched.
- If accepting/saving an edit fails, retain the last accepted values and report
  the error. If applying an accepted edit fails after quiescence, preserve its
  intent and recoverable artifacts, expose retry/error status, and do not report
  that the new configuration is active or synchronized.
- A failed older edit may restore controls only if there is no newer accepted
  edit. It must never undo the user's latest selection.
- Reopening Settings must distinguish saved desired configuration from active
  transition/error state rather than silently showing an obsolete snapshot.

## Required Acceptance Evidence

Use real input through `tests/common/interaction.rs` and
`tests/common/shell.rs`, plus a controllable provider holding a genuine run
in-flight. The checks in [`docs/testing.md`](../testing.md#cloud-sync-edit-first-acceptance)
are required before marking this contract implemented. An idle-only test or a
fixture pre-paused to make controls clickable is not evidence of compliance.
