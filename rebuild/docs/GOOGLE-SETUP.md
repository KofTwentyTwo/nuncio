# Set up Google on your Mac

Do this once for Nuncio. The same app registration can connect more than one
Google mailbox. You can use the currently available testing build with the
local JSON file below; no local build or GitHub configuration is required.

## 1. Create the Google project

Open [Google Cloud Console](https://console.cloud.google.com/) and sign in with
the Google account you want to own Nuncio's registration. Use the project picker
at the top, choose **New project**, and name it **Nuncio Testing**. Let Google
choose an available project ID. Use **No organization** if available; if your
Workspace requires a managed organization, use its approved location.
Select the new project before continuing.

## 2. Enable mail and calendar

Open **APIs & Services → Library**. Find and enable **Gmail API**, then
**Google Calendar API**. Both must be enabled in the same project.
[Google's API instructions](https://developers.google.com/workspace/guides/enable-apis).

## 3. Configure sign-in

Open **Google Auth Platform → Branding → Get Started**. Enter **Nuncio Testing**
as the app name and use your email for support and contact information. Choose
**External** for the audience and complete Google's setup prompts.

Under **Audience**, keep publishing status **Testing**. Under **Test users**,
add each Google email address you want to connect, including your own. The
registration owner and the mailbox you connect can be different accounts.
Under **Data Access → Add or Remove Scopes**, add and save:

```text
openid
https://www.googleapis.com/auth/userinfo.email
https://www.googleapis.com/auth/gmail.modify
https://www.googleapis.com/auth/calendar
```

Google's email scope corresponds to the `email` identity permission Nuncio
requests. Gmail and Calendar permissions include writes; they match Nuncio's
engine. [Google's consent setup instructions](https://developers.google.com/workspace/guides/configure-oauth-consent).

## 4. Download the Desktop client

Open **Google Auth Platform → Clients → Create client**. Choose **Desktop app**,
name it **Nuncio macOS Testing**, and click **Create**. Download the JSON from the
client details. Rename the downloaded file **nuncio-google.json**, leaving it in
**Downloads**. Nuncio manages the Desktop client's local browser callback.
[Google's Desktop client instructions](https://developers.google.com/workspace/guides/create-credentials#desktop-app).

Keep this file outside Git and out of chat/email. On your Mac, run:

```sh
chmod 600 "$HOME/Downloads/nuncio-google.json"
```

## 5. Connect in Nuncio

Keep the daemon running in one Terminal window:

```sh
~/.local/opt/nuncio-testing/bin/nunciod --profile laptop-qa
```

In a second Terminal window, run:

```sh
~/.local/opt/nuncio-testing/bin/nuncio-cli --profile laptop-qa account add \
  --client-config "$HOME/Downloads/nuncio-google.json"
```

Choose **Google**, enter the mailbox address you added as a test user, and
confirm the setup. Complete browser sign-in using that same identity and grant
the requested Gmail and Calendar permissions. If Google shows an unverified-app
testing warning, check that the app and project are the ones you just created.
An administrator-blocked consent request needs your Workspace administrator;
changing permissions or switching client types is not a fix.

Connecting starts background mail/calendar downloads. Account setup itself does
not send mail/invitations or change remote messages/events. Nuncio stores account
credentials in macOS Keychain; this JSON is the app registration. Retain the file
privately for another account or future reauthorization. Reusing it for setup
does not require registering another project.

Check the saved account:

```sh
~/.local/opt/nuncio-testing/bin/nuncio-cli --profile laptop-qa account list
```

Use the entry's `id` with individual-account commands. [Examples](ACCOUNT-MANAGEMENT.md#work-on-one-account).
Google External/Testing grants with these scopes can expire after seven days,
so browser reauthorization may be needed. [Google's token-lifetime guidance](https://developers.google.com/identity/protocols/oauth2#expiration).

## If setup stops

| Message or symptom | What to check |
|---|---|
| Google sign-in unavailable | Include `--client-config` with the downloaded Desktop JSON. |
| Invalid registration/file permissions | Choose the Desktop app JSON, verify its filename, and run `chmod 600` on that exact file. |
| Access denied or tester restriction | Add the mailbox under this project's Audience → Test users, and use that identity in the browser. |
| Gmail or Calendar permission missing | Enable both APIs and approve all requested scopes. |
| CLI cannot reach the daemon | Keep it running and use `--profile laptop-qa` in both terminals. |
| Browser did not open | Add `--no-browser` and open the displayed sign-in URL yourself. |

Future testing builds can include Nuncio's shared registration so normal setup
only needs `account add`. That maintainer step is described in
[GOOGLE-APP-REGISTRATION.md](GOOGLE-APP-REGISTRATION.md); it has not been performed.
Live acceptance remains separately recorded in [MANUAL-ACCEPTANCE.md](MANUAL-ACCEPTANCE.md).
