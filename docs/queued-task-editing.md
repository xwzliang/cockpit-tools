# Queued task editing and scheduling

- Pending and waiting-for-interruption Codex tasks (local or SSH) can be edited in place. The modal reuses the creation fields: title, prompt, model, environment/server, project, target thread, and queue-entry time.
- Queue-entry timestamps are stored as Unix seconds, edited/displayed as local 24-hour wall time, and accompanied by a live remaining-time indicator. A past time means eligible now, not a promise of immediate execution; quota, concurrency and thread dependencies still apply.
- Use a separate date field and explicit `HH:mm[:ss]` text field. Native `datetime-local`/`time` controls may display AM/PM according to OS locale, even if their values are ISO formatted.
- Discovery lists are suggestions: delayed project/thread responses must not replace saved or manually entered selections. Ignore responses from obsolete requests.
- Never save a stale full modal snapshot as authoritative queue state. The edit command copies only configurable fields from the request and preserves stored ID, order, creation time and execution metadata.
- The async edit/dispatcher gate protects the dispatcher's snapshot through launch and save. An edit waits for an in-progress dispatcher, then validates the current stored task is still pending/waiting. It must reject a task that has started or finished. Do not hold a standard mutex guard across an await or start a dispatcher while holding the edit gate.
- Manual validation is delegated to the user's Mac; no tests or builds were run for this change.

## Thread display names

Queue headings, thread selectors, and interrupted-task cards prefer the Codex sidebar name instead of the opening prompt. Local reads reuse the session display resolver (desktop catalog, then `threads.name`); SSH reads prefer nonempty `threads.name` and fall back to `threads.title` for older databases. Queue refresh resolves saved entries by thread ID on their own host, so existing entries and later renames are reflected without editing prompts or persisted task titles. The queue task title remains available in the heading tooltip and editor. If the remote host is unavailable, the saved thread label is retained.

Validation: `python3 scripts/test_codex_task_titles.py` covers remote selectors, saved-ID lookups, renames, blank/missing names, legacy schemas, and regular/goal interruptions. Local precedence is covered by `codex_session_display::tests`.
