# Queued message delivery mode

Queued tasks default to `delivery_mode: wait`; persisted tasks without this field retain existing behavior. For an existing local or SSH thread, `steer` attempts delivery to the current active turn after the configured queue-entry time. If no active turn exists, normal scheduling, concurrency limits, quota checks and interrupted-task recovery still apply. Steering an existing turn does not consume a new concurrency slot and may bypass messages waiting for completion.

The existing Codex control WebSocket must belong to the process running the target turn. A separate CLI `exec resume` process cannot steer that turn. The helper initializes the connection, reads live turns through `thread/read` with `includeTurns`, then invokes `turn/steer` with `expectedTurnId`. It sends only the prompt: the active turn's model, project and other settings remain unchanged. SSH uses the selected server's configured Codex home.

Reference: https://learn.chatgpt.com/docs/app-server#steer-an-active-turn

A successful acknowledgement is recorded as `steered` (message sent to the active turn), including `assigned_turn_id`. This is deliberately distinct from `completed`: it does not assert that the active turn has finished. Missing control sockets, unsupported APIs, turn-ID races, and uncertain transport outcomes are shown as errors. No CLI fallback or automatic resend follows a steer attempt, since a lost response may hide successful delivery. Inspect the target thread before manually retrying an uncertain result.

Durable rule: never turn a steer acknowledgement into turn completion, never apply model/cwd overrides to `turn/steer`, and never silently replace a failed/uncertain steer with `turn/start` or CLI execution. Re-read live turn state rather than trusting a potentially stale history snapshot.

Implementation was reviewed statically. No tests, builds or live steering calls were run, per the user's instruction to verify on their Mac.
