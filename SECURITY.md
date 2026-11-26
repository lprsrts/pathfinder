# Security

## Reporting a problem

Please report security problems privately, not in a public issue:

- through GitHub's private vulnerability reporting (the **Security** tab of this repository, "Report a vulnerability"), or
- by email to alper@saritas.io.

Say what you found, how to reproduce it, and what it affects. You'll get an answer within a week. Please give me time to release a fix before you publish details.

## What's in scope

- The latest release. Older versions get fixes only through `pathfinder update`.
- Ways out of the command sandbox (Seatbelt on macOS, bubblewrap on Linux), or ways for a sandboxed command to read credentials or reach the network without asking.
- The gate: replies that reveal a candidate's answer before your verdict, or a way to read a candidate's key.
- The record: ways to mark something as understood or vouched without the evidence the theory requires, or to alter the ledger without it showing.
- The update path: the release manifest signature, the checksums, the installer.
- The collector service: authentication, access to another install's data, the shared gate's limits, the sign-in flow.

Prompt injection that makes a model say something wrong is not a vulnerability by itself. It is one when it gets past a guarantee above.
