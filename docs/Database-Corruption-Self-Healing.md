# Database Corruption Detection & Self-Healing Engine

> **Status:** the detection/backup/salvage machinery is real and tested. The two
> defects tracked in [`BACKLOG.md`](BACKLOG.md) (Phase 1.B) — transient errors
> (pool timeout / `SQLITE_BUSY`) being mis-classified as corruption and deleting
> a live DB, and rule salvage targeting the wrong schema so filter rules were
> never restored — are both fixed (`crates/nuncio-store/src/recovery.rs`).

Nuncio features a 4-Stage Database Corruption Recovery and Self-Healing Engine.

---

## 1. 4-Stage Self-Healing Recovery Flowchart

*(No diagram is currently checked in for this pipeline — the image previously
embedded here was the NSQL filter compiler pipeline, not the self-healing
recovery flow, and has been removed as inaccurate.)*
