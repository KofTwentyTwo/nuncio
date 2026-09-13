# Guided account setup

James requested a genuinely simple account setup flow on September 13 after
successfully running the laptop quick start. The existing account engine/API
remain the authority; the CLI guides input and presents results. Implement inline
on the existing authorized feature branch. No native application or live account
access is included.

1. Add `account add`: provider choice, email, direct hidden password entry,
   sensible verified-TLS MailPlus defaults, optional advanced settings, explicit
   consent before connection/background synchronization, and clear success/errors.
   Cancellation and nonterminal/JSON input must not submit credentials. Retain
   existing scriptable commands and secure stdin interfaces.
2. Use a maintainer-supplied Desktop OAuth registration automatically in testing
   builds; retain an explicit developer registration override. A build without
   registration explains Google availability before requesting credentials or
   starting OAuth. James confirmed no registration exists: app registration and
   named live acceptance remain external steps, not per-user Cloud setup.
3. Add actual PTY-driven CLI/daemon tests against independent Google and MailPlus
   services: hidden secrets, cancellation/terminal restoration, failure recovery,
   consent/account identity, saved settings, restart, and unchanged remote effects.
   Run relevant offline suites and production/package checks; preserve RED evidence.
4. Update guides, session state and evidence. Sign/push coherent checked changes,
   verify eligible testing delivery, and give James the single guided command.
   Do not claim Google live compatibility or registration completion from mocks.

Google Desktop OAuth is a public-client flow using PKCE and the system browser.
Its app registration identifies Nuncio; it is distinct from per-account refresh
credentials in the OS keystore. Registration material stays out of source control
and is supplied explicitly for a build. See Google's
[native app flow](https://developers.google.com/identity/protocols/oauth2/native-app)
and [OAuth policies](https://developers.google.com/identity/protocols/oauth2/policies).
