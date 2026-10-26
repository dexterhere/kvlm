# Hugging Face access

Date: 2026-10-26. Applies to the Models page. Nothing here runs at startup.

## Public models

Search, file lists, header inspection and downloads of public repositories need no account
and no token.

## Gated or private models

1. On huggingface.co, sign in and accept the model's terms yourself. This application never
   accepts terms for you.
2. Create a read-only access token in your Hugging Face account settings.
3. On the Models page, open Hugging Face access, paste the token, and press Save. The token
   goes to your desktop keyring and is used immediately. The box is cleared.
4. Press Check to ask Hugging Face whose token it is. Saving does not do this by itself.
5. Next session, press Use saved to load it from the keyring. The keyring is not read until
   you press a button.
6. Press Remove to forget the token and delete the saved copy.

## When something is missing

| Message | Meaning |
| --- | --- |
| No keyring (Secret Service) is available | Install and start GNOME Keyring or another Secret Service. The token still works until you close the application. |
| The keyring is locked | Unlock it, then retry. |
| The keyring request was dismissed | You closed the prompt. Retry and approve it. |
| The keyring did not answer in time | Five second limit. If a prompt is open, answer it and retry. |
| Hugging Face did not accept the token | Wrong, expired, or revoked token. |
| This repository is gated or private | Accept the terms on huggingface.co and add a token. |

## What is sent where

The token is sent only to `huggingface.co` when fetching model files and when you press
Check. It is not sent with searches or file lists and is never forwarded to download
redirects. Details and the reasons are in
[ADR 0016](../05-decisions/0016-huggingface-token.md).

## Tests

Tests use an in-memory store and a local mock server, so they need no keyring, network or
account: token validation and redaction, save, load and clear consistency, keyring failures,
the timeout path, first-hop-only attachment including across a redirect, no token on listing
requests, and rejection handling for the token check.
