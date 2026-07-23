# Security Policy

Nuncio ([nuncio.mx](https://nuncio.mx)) takes security and data privacy seriously. Because Nuncio handles sensitive mail payloads, calendar schedules, and authentication credentials across Windows, macOS, and Linux, we maintain strict security disclosures and cryptographic standards.

## Supported Versions

| Version | Supported |
| :--- | :--- |
| `0.1.x` | Supported |
| `< 0.1.0` | Unsupported |

## Reporting a Vulnerability

**Do NOT report security vulnerabilities via public GitHub Issues.**

If you discover a security vulnerability in Nuncio, please report it privately:

- **Email**: Security reports should be emailed to `security@kof22.com` (or `james@kof22.com`).
- **Response Time**: We will acknowledge receipt of your report within 48 hours and provide a detailed assessment and remediation timeline within 7 business days.

## Security Practices in Nuncio

1. **Credential Vault Integration**: Nuncio never stores plain-text passwords or refresh tokens in configuration files or unencrypted SQLite tables. All authentication credentials are directed to OS native vaults (`keyring` crate) such as macOS Keychain, Windows Credential Manager, and Linux Secret Service (D-Bus).
2. **Untrusted HTML Sandboxing**: In the desktop GUI shell (`nuncio-gui`), untrusted HTML emails are rendered inside isolated `<iframe sandbox>` tags with JavaScript execution disabled, custom URI protocols (`nuncio-mail://`) proxying attachments, and remote tracking pixel loading blocked by default.
3. **Data-at-Rest Encryption**: Offline attachment blobs and local caches are encrypted at rest using `age` (X25519 / ChaCha20-Poly1305 streaming cipher).
