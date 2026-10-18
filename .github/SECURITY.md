# Security policy

## Supported versions

Only the latest release of Ashdash receives security fixes. If you can, check that the problem is still present in the newest
release (or on `main`) before you report it.

## Reporting a vulnerability

Please report vulnerabilities **privately** through GitHub's private vulnerability reporting:

1. Open the repository's **Security** tab.
2. Choose **Report a vulnerability**.
3. Fill in the form.

Do not open a public issue or pull request, and do not post exploit details in discussions, for a suspected vulnerability.

### What to include

* The Ashdash version (`ashdash --version`) and the macOS version and architecture.
* What the problem is and what an attacker gains (for example: runs code, reads or writes files, bypasses an import prompt).
* Steps to reproduce, ideally with a minimal synthetic workflow, theme or URL. Do not attach real personal data, secrets or
  workflows you have no right to share.
* How you found it, and whether it is already public.
* Any fix or mitigation you suggest.

## Scope

What counts as a vulnerability, and what the project already knows about, is described in [docs/SECURITY.md](../docs/SECURITY.md)
(the threat model and the status of each finding). Several behaviours of workflow packages are open by design and are shown
to the user in the import prompt; a report that only restates one of them is better filed as an ordinary issue.
