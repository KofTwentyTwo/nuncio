# Connect an account

Start your testing daemon, then open a second terminal in the installed directory:

```sh
./bin/nuncio-cli --profile laptop-qa account add
```

Choose **Google** or **Synology MailPlus / IMAP**. The command guides you through
setup and asks before connecting and enabling background synchronization.
Passwords are entered directly in the terminal with echo disabled; no JSON file,
shell variable, or password command argument is needed. Press Ctrl-C to cancel.

## Synology MailPlus / IMAP

Enter your email address, mail server hostname, username, and password. Press
Enter to use your email address as the username. The normal defaults are secure
IMAP on port 993 and SMTP with STARTTLS on port 587, using the same login.
Keep advanced settings unchanged if these match your server.

Choose advanced settings for different ports, SMTP login, a private CA certificate,
or mailbox names. Unencrypted connections and invalid TLS certificates are refused.
The default Sent folder is `Sent`, with a client-created Sent copy; select server
Sent-copy handling if your SMTP server creates its own copy. Confirm this choice
before testing sends to avoid duplicate Sent copies. Synology calendar services
are outside the IMAP/SMTP adapter.

The engine checks both logins before saving the account. If setup reports an
uncertain outcome after a timeout or interruption, run `account list` before
retrying. Ctrl-C before the final confirmation submits no sign-in.

## Google

In a Google-enabled build, enter your email address and finish sign-in in your
browser. Nuncio requests Gmail and Calendar access, including write permissions.
Check the selected Google identity before approving. The confirmed identity is
shown when connection succeeds. Connecting enables background reads; setup itself
does not send mail or invitations or modify remote messages/events.

**Current prerequisite:** Nuncio does not yet have a Google Desktop OAuth client
registration. Builds without it say that Google sign-in is unavailable before
asking for account details. Registering the Nuncio application is a one-time
maintainer task; users should not create Cloud projects for each account.
The build mechanism is described in [PACKAGING.md](PACKAGING.md). The maintainer
must also configure consent, enable Gmail/Calendar APIs, and admit named testers.
Google Testing-mode grants can require reauthorization after seven days.

Developers can override the bundled registration with `account add --client-config
/PRIVATE/desktop.json` (file mode 0600), or use `--no-browser` to open the displayed
URL manually. These are optional developer controls.

## After connecting

Use the same `--profile laptop-qa` on subsequent commands:

```sh
./bin/nuncio-cli --profile laptop-qa account list
./bin/nuncio-cli --profile laptop-qa account check --account ACCOUNT_ID
```

Existing account editing, pause/resume, recoverable removal, and explicit permanent
deletion are documented in [ACCOUNT-MANAGEMENT.md](ACCOUNT-MANAGEMENT.md). The guided
command requires an interactive terminal and refuses `--json`; existing
`add-google`, `add-imap`, and other scriptable commands remain available.

Automated verification uses synthetic local Google and independent IMAP/SMTP
services. Named live-account actions and native credential acceptance remain
separate checks in [MANUAL-ACCEPTANCE.md](MANUAL-ACCEPTANCE.md).
