# Database Corruption Detection & Self-Healing Engine

> **Status (2026-07-26):** the detection/backup/salvage machinery is real and
> tested, but has two known defects being fixed in [`BACKLOG.md`](BACKLOG.md)
> (Phase 1.B): transient errors (pool timeout / `SQLITE_BUSY`) are mis-classified
> as corruption and can delete a live DB, and rule salvage targets a wrong schema
> so filter rules are never restored. Treat as design intent pending those fixes.

Nuncio features a 4-Stage Database Corruption Recovery and Self-Healing Engine.

---

## 1. 4-Stage Self-Healing Recovery Flowchart

![Self Healing Pipeline](assets/nsql_pipeline.svg)
