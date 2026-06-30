# Security Policy

## Scope

H-Gripe Studio is a local-first desktop application (Tauri + Rust backend +
Python bridge). It runs on the user's own machine, stores everything under a
single local workspace (`user/hgripe`), and reaches the network only to call the
remote AI providers the user configures. Our threat model assumes:

- The user installed H-Gripe Studio through a supported channel: a release build
  or a build from source following the README.
- The Tauri webview content security policy (`tauri.conf.json`) is the one we
  ship; the front end is loaded locally, not from a remote origin.
- Credentials and provider profiles are managed through H-Gripe's credential
  refs / profiles, which keep API keys out of workflow files and history.
- The user has not deliberately exposed any local helper service to the network
  or wired the broker to an untrusted provider endpoint.

A report is in scope only if it affects a user operating within this threat model.

## What We Consider a Vulnerability

We want to hear about issues where a **reasonable user** — someone who reads UI
prompts and warnings before clicking through them — can be harmed by H-Gripe
Studio itself.

The clearest examples, using only built-in functionality:

- A workflow file (`WorkflowGraph` JSON) that such a user might plausibly open
  and run that results in **untrusted code execution**, **arbitrary file
  read/write outside the expected workspace/output directories**, or
  **credential/data exfiltration** (e.g. leaking `credentials.json`, inline API
  keys, or Authorization headers).
- A PSD or image input that, when processed by a built-in card / the Python
  bridge, escapes its sandboxed directories or executes code.
- Logs, history records, or `doctor`/diagnostics output that leak secret values
  that are supposed to be redacted.

When submitting a report, please explain *why this is a problem for a typical
local H-Gripe Studio user*. Reports without this context are difficult to act on.

## What We Do Not Consider a Security Vulnerability

Please report the following through our regular
[GitHub issues](https://github.com/tanzanite2025/H-Gripe-Studio/issues) instead.
Filing them as security reports will likely cause them to be deprioritized or
closed.

- **Issues that require the user to expose a local service to the network.**
  H-Gripe is local-first. If a remote attacker needs network access to a service
  the user chose to expose, securing that deployment (firewall, reverse proxy,
  authentication) is the user's responsibility. These are bugs, not
  vulnerabilities.
- **Issues that depend on the user configuring an untrusted or malicious
  provider endpoint / `base_url`.** The configured provider is trusted code; data
  sent to it is sent at the user's direction.
- **Vulnerabilities that depend on outdated dependency versions** that we neither
  ship nor recommend.
- **Crashes, hangs, or resource exhaustion from a loaded workflow or input
  image.** Annoying, but not a security issue in our model — file a regular bug.
  (Note: oversized decode inputs are already guarded via `--max-decode-pixels`.)
- **Social-engineering scenarios** where the user is expected to ignore an
  explicit UI warning or prompt.

## Reporting

If you believe you have found an issue that falls within the scope above, please
report it privately via GitHub's
[Report a vulnerability](https://github.com/tanzanite2025/H-Gripe-Studio/security/advisories/new)
feature rather than opening a public issue.

Please include:

1. A description of the vulnerability and the affected component (desktop app /
   `hgripe-api` broker / Python bridge).
2. Reproduction steps, ideally with a minimal workflow file or proof-of-concept.
3. The H-Gripe Studio version, install method (release / build from source), and OS.
4. An explanation of how this affects a typical local user as described in the
   threat model.

We will acknowledge valid reports and coordinate a fix and disclosure timeline
with you.
