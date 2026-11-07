# Contributing to FreedomBooks

Thank you for your interest in FreedomBooks. Here is how you can help right now.

## Contributions and DCO sign-off

FreedomBooks uses the [Developer Certificate of Origin 1.1](DCO) (DCO). No separate Contributor License Agreement (CLA) is required. Contributions are submitted under the project's AGPL-3.0-or-later license, including its [Electron/Chromium additional permission](LICENSE-EXCEPTION). You retain copyright in your contributions.

Read the DCO and add your sign-off to each commit you submit for inclusion. The sign-off certifies that you have the right to contribute under those terms. Git can add it for you:

```sh
git commit --signoff
```

This adds a trailer using your configured Git name and email:

```text
Signed-off-by: Your Name <you@example.com>
```

Only add your own sign-off, and preserve other contributors' authorship and sign-offs. Your contribution and sign-off become part of the public, retained project history. A DCO sign-off is a certification, not a cryptographic signature.

Maintainers check sign-offs before accepting contributed commits. Bug reports, questions and feature requests do not need a sign-off.

## What helps most today

- **Bug reports.** FreedomBooks is an early alpha, so every report helps. Use the [bug report form](https://github.com/CyberJody/FreedomBooks/issues/new?template=bug_report.yml).
- **Feature requests.** Tell us what you need and why. Use the [feature request form](https://github.com/CyberJody/FreedomBooks/issues/new?template=feature_request.yml).
- **Questions and ideas.** Ask in [Discussions](https://github.com/CyberJody/FreedomBooks/discussions). Answering other people's questions helps too.
- **Security problems.** Please report these privately, not in an issue. See [SECURITY.md](SECURITY.md).

Please search the existing issues first. If someone already reported the same thing, add a comment with your details or a thumbs-up instead of opening a new one.

## How to write a good bug report

A good bug report lets someone else see the problem on their own computer. Include:

1. **What you did.** The exact steps, one per line, starting from the Home screen. For example: "Home > Create invoice, pick a customer, add one line of 2 at $10.00, click Save".
2. **What you expected** to happen.
3. **What happened instead.** Copy any error message word for word.
4. **Whether it happens every time.** If not, how often.
5. **Your FreedomBooks version and operating system.** The app does not show its version yet, so tell us which release you downloaded, or the commit you built from (`git rev-parse --short HEAD`).
6. **A screenshot**, if it shows the problem.

Keep private data out of it. Use a test company with made-up names and amounts. Never post real financial data, bank details, passwords, keys or your company file. If the problem only happens with your real books, say so, and we will work out a safe way to look at it.

One problem per report, please. Short and specific is better than long.

[docs/REPORTING-PROBLEMS.md](docs/REPORTING-PROBLEMS.md) has the same advice in a step-by-step form for people new to GitHub.

## Running the tests

If you build from source, running the tests is a good way to check your setup or to confirm a bug. See the [README](README.md#build-from-source) for what to install.

Rust core, from the repository root:

    cargo test --workspace

On Linux, one sorting test (`fb-db --test collation`) checks names against the system's `en_US.UTF-8` locale, so that locale needs to be installed (on Debian or Ubuntu: `sudo apt install locales`, then enable `en_US.UTF-8` with `sudo dpkg-reconfigure locales`). Without it, that one test fails and everything else still runs.

Lints, the same way the project runs them:

    cargo clippy --workspace --all-targets -- -D warnings

Desktop app, from `electron/`:

    npm ci
    npm run build:native
    npm test

The end-to-end smoke test drives a real window with Playwright. On Linux without a display, run it under Xvfb:

    xvfb-run -a -s '-screen 0 1600x1200x24 -nolisten tcp' npm run smoke

`scripts/gate.sh` is the full check the project runs before merging. Its last step, the screen diff, compares screens with the v1 app and needs PostgreSQL and a copy of the v1 reference app, so most people will not be able to run that part. The commands above cover everything else.

The gate has two tiers:

- `scripts/gate.sh --quick` is for working on a change. It runs clippy, the tests of the crates you changed since your branch left `main` (set `FB_GATE_BASE` to compare with another branch) plus every crate that depends on them, and the check that `fb-core` stays free of Node and network crates. It skips the JSON feature run and the screen diff. Changes outside the crates (scripts, `electron/`, docs) test no crate; a change to the root `Cargo.toml`, `Cargo.lock` or another workspace-wide build file tests every crate. It prints which crates it tests and why. Its own tests: `python3 scripts/tests/test_gate_quick.py`.
- `scripts/gate.sh` with no flag is the full gate. Run it once before a merge.

If a test fails on a clean checkout, that is a bug worth reporting. Include the command you ran, the failing test's name and its output.

## Code of conduct

Be kind and patient. Many people here are bookkeepers and business owners, not programmers. Explain things plainly, and assume good intent.
