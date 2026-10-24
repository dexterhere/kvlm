// SPDX-License-Identifier: GPL-3.0-or-later
//! Metadata integration is deferred. The inspected llmfit-core 1.1.17 API uses
//! heuristic weights/KV estimates rather than the explicit byte/block contract of
//! `memory_math`. Missing or unsupported metadata must remain unknown until a
//! validated metadata adapter exists; no architecture table is copied here.
