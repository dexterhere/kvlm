# 0016: Hugging Face access token

Date: 2026-10-26. Status: Phase 5 of the [M1 plan](../04-roadmap/m1/Implementation.md)
implemented.

## Decision

Gated and private Hugging Face repositories need an account token. The application keeps
that token only in the desktop keyring (the freedesktop Secret Service) and in memory for
the running session. It is never written to a file, SQLite, a log, an error message, an
event, or an environment variable, and it is never read from one.

Nothing contacts the keyring at startup. The keyring is used only when the user presses
Save, Use saved, or Remove in the Hugging Face access panel. Whether a keyring exists is
learned from the first such press, not probed in advance.

The application never accepts a model's terms. A `401` or `403` becomes an access message
that tells the user to accept the terms on huggingface.co with their own account and to add
a token.

## Keyring

- `oo7` 0.6 (MIT) provides the Secret Service client with the `tokio` and `native_crypto`
  features. It adds about sixty permissively licensed crates (zbus, ashpd and their
  dependencies); no new `unsafe` enters this workspace.
- Only the encrypted transport (`Service::encrypted`) is used. `oo7`'s `Service::new`
  would silently fall back to a plain transport, which would send the token unencrypted over
  the session bus; this application refuses that and reports the keyring as unavailable.
- Items carry the attributes `application=org.gnome.LocalAIWorkstation` and
  `service=huggingface`. The secret is the token only.
- Every keyring call is limited to five seconds. A locked collection may show an unlock
  prompt; if the prompt is not answered in time the action reports a timeout and can be
  retried. Keyring work runs on its own single-flight task, so a hung prompt cannot block
  downloads or browsing.
- The sandbox (Flatpak portal) file backend is not used. Sandboxed packaging decides this in
  M4.
- If the keyring is missing, locked, or dismissed, the token can still be used for the
  running session, and the message says that it was not saved. There is no fallback store.

## The token value

`Token` accepts only letters, digits, `_`, `-` and `.` up to 256 characters, after trimming
surrounding whitespace. That keeps it safe in an HTTP header and out of malformed input.
`Debug` and `Display` print `[redacted]`, and the value is zeroized when dropped. Pasted text
is cleared from the entry as soon as it is read. GTK and the D-Bus library keep their own
copies while a call is in flight; this is a known limit of those libraries.

## Where the token is sent

- To `https://huggingface.co` (default port, no URL credentials) only, on the first hop of
  a file request (header inspection and download) and on `GET /api/whoami-v2`.
- Never to catalog search or file listing requests, which work without a token.
- Never after a redirect. Signed CDN links and `hf.co` hosts receive no `Authorization`
  header.
- Token checks run only when the user presses Check. Saving does not validate.

## Consequences

- Without a keyring, gated downloads still work for a session if the user pastes a token
  each time.
- "Saved" and "removed" are reported only after the keyring confirms. Removal forgets the
  in-memory token first, so a failing keyring never leaves a removed token in use.
