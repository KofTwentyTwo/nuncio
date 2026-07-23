# Systems Architecture & Topology Specification

Nuncio is engineered as a **Library-First, Daemon-Centered Sovereign Communication Suite** designed for high throughput, low latency (<3.2ms search), and strict domain isolation across all 10 workspace crates.

---

## 1. System Topology & Process Boundaries

![System Context Diagram](assets/c4_context.svg)

---

## 2. IPC Framing & Message Sequence Flow

Connected applications communicate with `nunciod` over local sockets using 4-byte Big-Endian length-prefixed JSON-RPC 2.0 frames:

![IPC Frame Sequence Flow](assets/ipc_sequence.svg)

---

## 3. Storage Architecture & Encryption Enclave

![Nuncio Architecture Topology](assets/topology.svg)
