# One-time Google registration

Status: proposed, awaiting explicit approval. The owner has been supplied in the
private local approval record. Creating this registration lets testing builds
offer browser sign-in through `nuncio-cli --profile laptop-qa account add`.
Accounts reuse this registration; they do not need separate Cloud projects.

## Proposed settings

| Setting | Value |
|---|---|
| Project and consent-screen app name | `Nuncio Testing` |
| Project ID | `nuncio-testing-20260913`; availability is unverified, permit a uniqueness suffix |
| Project location | `No organization` if available; obtain approval for a mandatory organizational parent before creation |
| Owner, support and developer contact | The designated Google account, recorded locally |
| Audience / publishing status | External / Testing |
| Initial OAuth test-user allowlist | Propose the designated owner only; addition requires the setup approval |
| Enabled APIs | Gmail API (`gmail.googleapis.com`) and Google Calendar API (`calendar-json.googleapis.com`) |
| OAuth client | Desktop app, named `Nuncio macOS Testing` |
| Requested scopes | `openid`, `email`, `https://www.googleapis.com/auth/gmail.modify`, `https://www.googleapis.com/auth/calendar` |

These scopes match `crates/nuncio-engine/src/providers/google/http.rs`. They
support identity checking, mail reads/writes/sending and Calendar reads/writes.
The engine uses PKCE and a numeric loopback callback with a dynamic local port.
External allows separately approved testers outside the owner's organization;
Internal would limit the audience to that organization. External Testing issues
refresh tokens that expire after seven days for these scopes, requiring browser
reconnection. This is a testing configuration, not a verified public launch.
See [Google Auth Platform](https://support.google.com/cloud/answer/15544987),
[desktop OAuth](https://developers.google.com/identity/protocols/oauth2/native-app)
and [token expiry](https://developers.google.com/identity/protocols/oauth2#expiration).

## Execution after approval

1. Use the owner's signed-in Google Cloud browser session. The owner handles any
   password or MFA entry directly. Create only the project/settings above. Record
   the actual project ID, parent and client ID; no billing link, organization-wide
   policy changes, extra IAM members or service accounts are included.
2. Create the [Desktop app client](https://developers.google.com/workspace/guides/create-credentials#desktop-app),
   download its `installed` JSON to a private local file outside Git, and enforce
   directory mode `0700` and file mode `0600`. The local approval record names
   the destination. Keep the JSON and credential values out of logs and chat.
3. With explicit repository-setting approval, create
   `NUNCIO_GOOGLE_DESKTOP_CLIENT_JSON` in `KofTwentyTwo/nuncio` using secure file
   input. If that secret already exists, inspect metadata and obtain replacement
   approval instead of overwriting it. Desktop client material is bundled into
   the downloadable CLI; it is distinct from account tokens held in Keychain.
4. Push the checked registration-completion documentation checkpoint to
   `feature/nuncio-google-first-rebuild` without `[skip ci]`. The existing workflow
   accepts the registration only on a push to this exact branch;
   `workflow_dispatch` will not include it. Review the selected run's actual
   results, download the artifact, verify its manifest and
   `google_oauth.configured=true`, then check the installer in a temporary prefix.
   All automated provider tests remain offline with synthetic credentials.
5. Send the short update/account-add instructions after the Google-enabled
   artifact passes. Named mailbox/calendar actions and native Keychain acceptance
   remain governed by [MANUAL-ACCEPTANCE.md](MANUAL-ACCEPTANCE.md); registering an
   OAuth tester does not authorize Nuncio to access that account.

Stop at any missing owner login, required organizational parent, policy override
or unexpected existing resource. Record the exact condition and continue any
independent offline work. If setup fails, record created resource IDs and leave
them unchanged pending instructions; rollback can remove the new repository
secret and OAuth client after approval. Do not delete an existing project.

Expected effort: 0.5–1 active hour for registration and build verification,
excluding approval, sign-in and CI waiting time. Live acceptance remains separate.
