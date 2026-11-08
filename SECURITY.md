# Security

## Scope

RsRazer controls devices locally. The application fetches device artwork and checks GitHub for
updates; device control uses the local daemon:

- The daemon binds **127.0.0.1 only** and refuses any other bind address.
- The listening socket asks for `SO_EXCLUSIVEADDRUSE`, not `SO_REUSEADDR`. Windows, unlike Unix,
  lets a second process bind a port that is already being listened on, so the usual `SO_REUSEADDR`
  default would allow any process running as the user to hijack the socket.
- Each run generates a random bearer token, written to `%LOCALAPPDATA%\rsrazer\daemon.json`
  with owner-only permissions. Every request must carry it.
- The RPC surface is a **fixed allowlist** of device methods.
- Recipe expressions are evaluated by a restricted AST walker that permits only arithmetic,
  comparisons and reads of the request buffer and device response. `eval` is never called.

Anything that can read the endpoint file can already act as the user, so the token defends against
other users on the machine and against local network clients, not against the user themselves.

## Reporting a vulnerability

Open a [security advisory](https://github.com/imacte/RsRazer/security/advisories/new), or an
issue if the problem is not sensitive. Please include what you ran, what you expected and what
happened.

## Hardware safety

The port sends the same commands the Linux kernel driver sends, built by the same logic, so it
carries the same risk profile as upstream OpenRazer: it writes lighting, DPI and polling settings,
and never touches firmware.
