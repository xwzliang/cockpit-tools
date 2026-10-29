# Automatic restoration grace period

Codex and Antigravity automatic restoration (including account-switch recovery) waits 60 seconds before dispatch. The existing per-task **自动恢复 / Auto restore** checkbox cancels the pending attempt when unchecked. Rechecking cannot reuse the old deadline; the next eligible scheduler pass starts a new full minute. Manual **Resume Now** remains immediate and invalidates a pending automatic attempt.

The backend owns countdown deadlines. The UI reads a snapshot and listens for pending changes, with polling as a fallback. Displaying or reopening a section does not start or reset a countdown. Pending deadlines are intentionally not persisted, so an app restart never resumes an expired deadline immediately.

Implementation rules:
- Keep cancellation separate from launch failures; cancelling must not spend the retry budget or cooldown.
- Invalidate the pending token on checkbox changes, dismissal, manual launch, and global disable. A rapid uncheck/recheck must not authorize the old pending attempt.
- Recheck task activity and eligibility after the grace period and cancellation tokens after quota or remote-auth awaits.
- Retain the existing per-task checkbox; do not add redundant pause/resume/cancel buttons.

This change was prepared without running tests or builds at the user's request. Native behavior should be checked on the user's Mac.

## Account-switch async dependency

The shared restore loop must not call automatic account switching. Account switching spawns the post-switch restore callback; having that callback await a loop which calls switching again creates a recursive opaque-future `Send` dependency and prevents compilation. The restore-only loop reports quota exhaustion to its caller. Only the periodic scheduler wrapper initiates another switch; the post-switch callback defers to the next scheduler pass. Keep both entry points on the same countdown/cancellation loop without introducing this dependency cycle.
