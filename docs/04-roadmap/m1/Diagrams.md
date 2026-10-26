# M1 diagrams

Companion documents: [Planned](Planned.md) · [Implementation](Implementation.md)

## Entities

```mermaid
erDiagram
    CATALOG_QUERY ||--o{ CATALOG_ARTIFACT : "returned"
    CATALOG_ARTIFACT ||--o| ARTIFACT_METADATA : "inspected"
    CATALOG_ARTIFACT ||--o{ DOWNLOAD : "copied into"
    CATALOG_ARTIFACT }o--o{ LOCAL_FILE_DIGEST : "matched by sha256"
    CATALOG_QUERY {
        text query_key PK
        int fetched_at_unix_ms
    }
    CATALOG_ARTIFACT {
        text artifact_id PK
        text repo_id
        text revision
        text filename
        int size_bytes
        text sha256
        text quantization
        text license
        text gated
    }
    ARTIFACT_METADATA {
        text artifact_id PK
        text gguf_json
        int fetched_at_unix_ms
    }
    DOWNLOAD {
        int id PK
        text artifact_id FK
        text state
        int bytes_done
        text expected_sha256
    }
    LOCAL_FILE_DIGEST {
        text path PK
        int size
        int mtime_ns
        int inode
        text sha256
    }
```

## Search and browse

```mermaid
sequenceDiagram
    actor U as User
    participant UI as Catalog view (GLib)
    participant B as Bridge
    participant H as HF client (Tokio)
    participant DB as SQLite cache
    U->>UI: Search "qwen coder"
    UI->>B: CoreCommand::SearchCatalog{id, query}
    B->>H: GET /api/models (repos only, bounded, timeout)
    alt success
        H-->>B: page of repos
        B->>DB: upsert cache (query_key, artifacts)
        B-->>UI: UiEvent::CatalogPage{id}
    else network or 429
        B->>DB: read cache
        B-->>UI: UiEvent::CatalogError{id, retry_after, cached}
    end
    UI->>UI: drop event if id is stale
```

## Artifact inspection

```mermaid
sequenceDiagram
    actor U as User
    participant UI as Catalog view
    participant B as Bridge
    participant H as HF client
    U->>UI: Inspect artifact
    UI->>B: InspectArtifact{id, artifact_id}
    B->>H: GET resolve/{revision}/{file} Range 0..16MiB
    H-->>B: header bytes + Content-Range total
    B->>B: total == catalog size? inspect_prefix, assess fit
    B-->>UI: ArtifactInspected{id, metadata, fit or Unknown}
```

## Deduplication

```mermaid
sequenceDiagram
    participant B as Bridge (spawn_blocking)
    participant S as Scanner
    participant DB as digest cache
    B->>S: scan roots (Ollama blobs, HF hub)
    S->>S: regular files only, stay inside root
    S->>S: size == artifact size ? candidate
    S->>DB: (path,size,mtime,inode) cached digest?
    alt miss
        S->>S: stream SHA-256, check cancel
        S->>DB: store digest
    end
    S-->>B: matches where sha256 == artifact sha256
```

## Download

```mermaid
sequenceDiagram
    actor U as User
    participant B as Bridge
    participant D as Downloader
    participant F as Filesystem
    participant H as Hugging Face
    U->>B: StartDownload{artifact_id}
    B->>D: begin (requires sha256, no shard)
    D->>F: take models/.lock, open <sha>.part
    D->>D: hash existing prefix, set offset
    D->>H: GET resolve/{rev}/{file} Range offset-
    alt 206 and Content-Range matches
        H-->>D: stream
    else 200 (range ignored)
        D->>F: truncate, offset 0
    else 401 or 403
        D-->>B: AccessRequired
    end
    loop chunks
        D->>F: write, update hash
        D-->>B: progress (throttled)
    end
    D->>D: final SHA-256 == expected?
    alt match
        D->>F: fsync, rename to blobs/sha256-<hex>
    else mismatch
        D->>F: delete part
        D-->>B: Failed(Integrity)
    end
```

## Download lifecycle

```mermaid
stateDiagram-v2
    [*] --> Queued
    Queued --> Running
    Running --> Paused: user pause or app exit
    Paused --> Running: resume
    Running --> Verifying: all bytes received
    Verifying --> Completed: digest matches
    Verifying --> Failed: digest mismatch
    Running --> Failed: network, disk, access
    Failed --> Queued: retry
    Running --> Cancelled: user cancel (part file kept)
    Cancelled --> Queued: restart
    Cancelled --> [*]: discard part
    Completed --> [*]
```

## Token handling

```mermaid
sequenceDiagram
    actor U as User
    participant UI as Preferences
    participant B as Bridge
    participant K as Secret Service
    participant H as Hugging Face
    U->>UI: Paste token
    UI->>B: StoreToken{secret}
    B->>K: store (attributes only, no label secrets)
    B-->>UI: TokenState{stored}
    U->>UI: Validate (optional, explicit)
    UI->>B: ValidateToken
    B->>H: GET /api/whoami-v2
    H-->>B: ok or 401
    B-->>UI: TokenState{valid or invalid}
    Note over B,H: token attached only to huggingface.co requests
```
