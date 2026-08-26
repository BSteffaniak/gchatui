# Google Cloud setup

The first release will use a desktop OAuth client supplied by each user. Detailed
steps will be verified during OAuth implementation. The intended setup is:

1. Create or select a Google Cloud project.
2. Enable the **Google Chat API** and **People API**.
3. Configure an OAuth consent screen for the intended test audience.
4. Create a Desktop application OAuth client.
5. Download the client configuration to a private local path.
6. Configure gchatui to read that path without copying the file.

- User-authenticated Google Chat responses intentionally omit other users' display
  names. gchatui resolves stable `sender.name` IDs through Google People API using
  read-only directory access. If directory lookup is unavailable or blocked, the
  message remains readable with a stable nonblank fallback label.
- The first release does not render Google Chat cards, widgets, attachment previews,
  annotations, or every rich-text semantic. It shows plain text and an explicit
  terminal notice when unsupported rich content is present.
- Space listing follows Google Chat API visibility rules and can omit conversations
  the API does not return, including some empty group chats and direct messages.
- Message updates are REST-based with manual refresh; Workspace Events/Pub/Sub
  real-time delivery is outside the first release.

Only read-only Chat and directory scopes will be requested. Organization policy can require an
administrator to approve an OAuth client even though the application does not make
company-wide configuration changes.

Never commit the downloaded client file or authorization output.
