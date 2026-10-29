# Queued task editing and scheduling

- Pending and waiting-for-interruption Codex tasks (local or SSH) can be edited in place. The modal reuses the creation fields: title, prompt, model, environment/server, project, target thread, and queue-entry time.
- Queue-entry timestamps are stored as Unix seconds, edited/displayed as local 24-hour wall time, and accompanied by a live remaining-time indicator. A past time means eligible now, not a promise of immediate execution; quota, concurrency and thread dependencies still apply.
- Use a separate date field and explicit `HH:mm[:ss]` text field. Native `datetime-local`/`time` controls may display AM/PM according to OS locale, even if their values are ISO formatted.
- Discovery lists are suggestions: delayed project/thread responses must not replace saved or manually entered selections. Ignore responses from obsolete requests.
- Never save a stale full modal snapshot as authoritative queue state. The edit command copies only configurable fields from the request and preserves stored ID, order, creation time and execution metadata.
- The async edit/dispatcher gate protects the dispatcher's snapshot through launch and save. An edit waits for an in-progress dispatcher, then validates the current stored task is still pending/waiting. It must reject a task that has started or finished. Do not hold a standard mutex guard across an await or start a dispatcher while holding the edit gate.
- Manual validation is delegated to the user's Mac; no tests or builds were run for this change.
