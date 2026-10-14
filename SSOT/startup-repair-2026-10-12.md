# Interactive startup repair

The ordinary interactive launch exits before opening when no model key is configured. Preserve the inherited working tree in a separate commit before editing.

Keep the shared chat screen open when settings are missing or unreadable. Validate settings before each task. Add /setup guidance and /reload; never accept or display keys in chat. A failed reload blocks new work until corrected and preserves the displayed settings. Preserve unsent requests, including /plan intent. Keep noninteractive validation unchanged.

Verify empty settings, malformed settings, recovery, all interactive entry paths, configured behavior, and an actual local model task. Use the existing terminal crate checks and command flow suite; no dependency changes.

## Verified result

The repaired program is installed at /Users/floyd/.local/bin/chimera.
Ordinary login-shell lookup resolves that executable. Start the chat with
`chimera tui`. Its saved settings use the existing local Bonsai model at
127.0.0.1:12434, with no remote model key.

Final executable SHA-256:
5cf9822f88a77ea2915668115ad0428f1e99b3492520ebf9fe4bf8ca42dbb024

Checks against the final executable: 72 terminal unit tests, strict terminal
code checks, formatting, CLI build, 79 setup/recovery checks, and 105 existing
screen-flow checks passed. Five setup launches exited normally and restored
terminal settings. Missing and malformed settings, unsent requests, reload,
initial request recall, and saved-session reopening were exercised.

The installed program also completed a real local-model task: read a random
marker from input.txt, show and receive approval for the exact result.txt write,
write it, read it back, save the answer, quit, and reopen the conversation.
All 19 final real-task assertions passed. Reopening added only the expected
resume event. No extra model or tool work occurred.

The original prompt asked the model to wait before writing, so it first asked
for approval in chat; a follow-up invoked the actual tool approval. Earlier
check-script failures from wrapped terminal text, redraw timing, and an overly
broad resume assertion are preserved in the evidence repository. These were
corrected without weakening the application checks.

Evidence repository:
 /Users/floyd/Documents/Codex/2026-10-11/you-know-what-lefty-you-dont
Final summary: outputs/chimera-repair-verification.json
Real task: work/chimera-real-task/report.json
Setup checks: work/setup-checks/chimera-setup-mzmf8e0j/report.json

This verifies the startup repair and these workflows. It is not a claim that
every CHIMERA feature or another model service has been exercised.
