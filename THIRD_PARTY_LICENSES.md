# Third-Party Open Source Software Acknowledgments & Licenses

Nuncio is built upon world-class open-source software libraries. We gratefully acknowledge the authors and maintainers of the following third-party dependencies:

---

## 1. Core Rust Backend & Protocol Libraries (`crates/*`)

| Library / Crate | License | Author / Copyright | Description |
| :--- | :--- | :--- | :--- |
| **`tokio`** | MIT | Tokio Contributors | Event-driven asynchronous I/O runtime engine |
| **`serde` / `serde_json`** | MIT / Apache 2.0 | Erick Tryzelaar & Ollie Charles | Fast zero-copy serialization framework |
| **`sqlx`** | MIT / Apache 2.0 | Launchbadge & SQLx Contributors | Async, type-safe SQLite database driver |
| **`lettre`** | MIT | Lettre Contributors | Email creation and SMTP transport client |
| **`async-imap`** | MIT / Apache 2.0 | Jon Gjengset & async-imap Authors | Asynchronous IMAP protocol client |
| **`aes-gcm`** | MIT / Apache 2.0 | RustCrypto Developers | Pure Rust AES-256-GCM authenticated encryption |
| **`age`** | MIT / Apache 2.0 | Filippo Valsorda & age Authors | Secure file and attachment stream encryption |
| **`zeroize`** | MIT / Apache 2.0 | RustCrypto Developers | Secure heap memory wiping on object drop |
| **`keyring`** | MIT / Apache 2.0 | Brogan Evans & keyring Authors | OS-native credential storage (Keychain, Credential Manager, Secret Service) |
| **`thiserror`** | MIT / Apache 2.0 | David Tolnay | Standard error trait derive macros |
| **`clap`** | MIT / Apache 2.0 | Clap Developers | Command-line argument parser for Rust |

---

## 2. Terminal TUI & GUI Desktop Libraries

| Library / Crate | License | Author / Copyright | Description |
| :--- | :--- | :--- | :--- |
| **`ratatui`** | MIT | Ratatui Developers | Terminal User Interface (TUI) rendering library |
| **`crossterm`** | MIT | Timon Post & Crossterm Authors | Cross-platform terminal manipulation library |
| **`tauri` (v2)** | MIT / Apache 2.0 | Tauri Apps Contributors | Ultra-lightweight desktop app framework |

---

## 3. React Desktop GUI Frontend (`crates/nuncio-gui/ui`)

| Package / Library | License | Author / Copyright | Description |
| :--- | :--- | :--- | :--- |
| **`react` / `react-dom`** | MIT | Meta Platforms, Inc. & React Contributors | User interface component framework |
| **`lucide-react`** | ISC | Lucide Contributors | Modern, clean icon library for UI |
| **`vite`** | MIT | Evan You & Vite Contributors | Frontend dev server and bundler |
| **`typescript`** | Apache 2.0 | Microsoft Corporation | Typed JavaScript language compiler |

---

## 4. Full License Texts

### MIT License
```text
Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### Apache License Version 2.0
```text
Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, EXPRESS OR IMPLIED,
See the License for the specific language governing permissions and
limitations under the License.
```
