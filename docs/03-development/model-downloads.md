# Model downloads

Date: 2026-10-26. Applies to the Models page. A download is an explicit action; the
application makes no network request at startup and never resumes a transfer on its own.

## Download a file

1. Search and expand a repository, then choose a file. The Download button is enabled
   only when Hugging Face publishes both a size and a SHA-256 for a single (not split)
   file. Otherwise the tooltip explains why it is disabled.
2. Select Download. The file is stored under
   `$XDG_DATA_HOME/local-ai-workstation/models/blobs/` after its SHA-256 matches. Until
   then it lives in `partial/` and is not usable.
3. The Downloads section above the results shows progress, and Pause, Cancel, Resume and
   Discard. Pause and Cancel keep what was downloaded. Discard deletes the partial file
   and needs a second click.

Only one download runs at a time. Closing the application pauses it (after a crash the
next start corrects the row); reopening lists it as paused. Nothing continues until you select Resume.

## What can stop a download

| Reason | What you see | What is kept |
| --- | --- | --- |
| Free space below the 2 GiB reserve | Paused, with an explanation | Partial file |
| Network problem or rate limit | Failed, with the reason | Partial file |
| Gated repository | Failed, access needed | Partial file |
| Server sends more than the published size | Failed | Nothing (untrusted) |
| Checksum mismatch | Failed | Nothing |
| Another application instance holds the store | Failed, store in use | Everything |

Gated repositories need an access token and acceptance of the terms on huggingface.co;
token support arrives with gated-model access. The application never accepts terms for
you.

## Limits

2 GiB free-space reserve, 64 GiB of partial files in total, progress updates at most four
times a second, bookkeeping saved every two seconds. Details and the safety reasoning are
in [ADR 0015](../05-decisions/0015-resumable-downloads.md).

## Tests

Downloader tests use a local mock server and temporary directories. They need no network,
GPU or model files: fresh transfer, resume with `206`, a server that ignores `Range`, bad
`Content-Range`, an overlong stream, a checksum mismatch, an already complete partial file,
an already published blob, a locked store, typed HTTP errors, pause and cancel, the space
and budget guards, and reconciliation after a simulated crash.
