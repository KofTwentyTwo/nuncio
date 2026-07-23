# The Four Great Interfaces

Nuncio guarantees **100% Feature Parity** across four distinct, decoupled user interfaces:

![Four Great Interfaces Topology](assets/topology.svg)

---

## 1. Interface Capabilities Comparison Matrix

| Capability / Feature | POSIX CLI | Vim Ratatui TUI | Desktop GUI | Native MCP AI |
| :--- | :---: | :---: | :---: | :---: |
| **Mail Read / Send / Search** | `nuncio mail ...` | Full Vim Split View | Rich Media Inspector | `nuncio_mail_send` |
| **Calendar Event Booking** | `nuncio cal ...` | `AppMode::Calendar` | Interactive Calendar | `nuncio_cal_create_event` |
| **NSQL Filter Engine** | `nuncio filter ...` | `[s]` Editor + `[t]` Preview | Visual Rule Builder | `nuncio_filter_create` |
| **Daemon Telemetry** | `nuncio daemon top` | Status Header Bar | Telemetry Modal | `nuncio_daemon_status` |
| **Auto-Update Engine** | `nuncio self-update` | Header `[u]` Chord | `<UpdateBanner />` CTA | `nuncio_update_apply` |

---

## 2. Keyboard Motions & TUI Navigation Flow

- **Vim Motions**: `j` (down), `k` (up), `h` (left), `l` (right), `g i` (go inbox), `e` (archive), `s` (syntax editor), `f` (filter rules), `u` (update).
