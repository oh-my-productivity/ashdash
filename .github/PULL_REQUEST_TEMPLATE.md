<!-- The pull request is squash-merged and its TITLE becomes the commit on main, so the title must follow Conventional Commits:
     type(scope): subject   e.g.  feat(gui): add a dark appearance   (types: feat fix perf refactor docs test build ci chore style revert;
     "!" after the type or scope marks a breaking change). CI checks the title and every commit message (job commit-lint). -->

## What and why

<!-- What changes, and why. Link the issue: "Closes #123". -->

## How I tested it

<!-- `scripts/check.sh`, and the CI runs you dispatched with `scripts/ci-run.sh` (paste the run URLs); what you could not test (a real keyboard, a second display, a permission prompt, ...). -->

## Checklist

- [ ] The title is `type(scope): subject` (lower-case, imperative, at most 72 characters); `!` or a `BREAKING CHANGE:` footer if it breaks something
- [ ] `scripts/check.sh` prints `check.sh: OK` (it is the only thing that runs on your machine besides `cargo test`; [docs/TESTING.md](../docs/TESTING.md))
- [ ] Anything end-to-end, GUI, stress or performance was run **on CI**, not locally: `scripts/ci-run.sh <suite> --watch`; the run URL is in "How I tested it". New slow or real-OS tests are `#[ignore = "ci: ..."]` with a fast variant
- [ ] Tests added or updated for a behaviour change; the matching page in `docs/` changed for anything a user can see
- [ ] No secrets, personal data or real user data; commits use a `users.noreply.github.com` address (the e-mail of a commit is public)
- [ ] New dependencies are pinned exactly and `scripts/gen-notice.py` was re-run (if dependencies changed)
- [ ] Security impact described below, if any (report a vulnerability privately first: `.github/SECURITY.md`)

## Notes for the reviewer

<!-- Risky areas, follow-ups, screenshots without anyone's desktop in them. -->
